# /// script
# requires-python = ">=3.10"
# dependencies = ["unicorn==2.1.4"]
# ///
"""Execute the built ARM image against UART and timer capture register models."""

import argparse
import binascii
import struct
from collections import deque
from pathlib import Path

from unicorn import UC_ARCH_ARM, UC_HOOK_CODE, UC_MODE_ARM, UC_PROT_EXEC, UC_PROT_READ, Uc
from unicorn.arm_const import UC_ARM_REG_CPSR, UC_ARM_REG_R0, UC_ARM_REG_R1


# Independent fixture for the five board paths: RX pin, timer, channel, TX pin.
PORTS = [(4, 0, 1, 28), (11, 1, 1, 13), (6, 0, 2, 22), (10, 1, 0, 8), (2, 0, 0, 29)]


def smoke(image: bytes, ram: bool, tx_port: int) -> None:
    rx_pin, tx_timer, tx_channel, tx_pin = PORTS[tx_port]
    # One inactive, dominant line is independent of the port under test.
    silent_port = 3 if tx_port == 4 else 4
    silent_pin, silent_timer, silent_channel, _ = PORTS[silent_port]
    cpu = Uc(UC_ARCH_ARM, UC_MODE_ARM)
    if not ram:
        cpu.mem_map(0, 0x10000, UC_PROT_EXEC | UC_PROT_READ)
    cpu.mem_map(0x40000000, 0x8000)
    base = 0x40001000 if ram else 0
    cpu.mem_write(base, image)
    cpu.mem_map(0x7FFFF000, 0x1000)

    # Outside the explicit TX command, only panel GPIO operations are allowed.
    cpu.mem_map(0xE01FC000, 0x1000)
    cpu.mem_map(0xE002C000, 0x1000)
    cpu.mem_write(0xE01FC088, struct.pack("<I", 1 << 10))  # PLL0 locked
    tx_body = bytes.fromhex("12300001a5")
    tx_check = binascii.crc_hqx(bytes([tx_port]) + tx_body, 0xffff)
    tx_cmd = f"A\nTX {tx_port} {tx_body.hex()} {tx_check:04x}\n".encode()
    frames = deque([b"?", b"A!", b"A\nBAD-ENTER-ISP\n", b"A\nID?\n", None, "inputs",
                    b"A\nLED-TEST\n", b"A\nPANEL?\n", b"A\nRX-END\n", b"A\nRX-BEGIN\n", b"A\nRX-BEGIN\n",
                    b"A\nRX-END\n", b"A\nRX-STREAM\n", tx_cmd, b"A\nRX-BEGIN\n", b"A\nENTER-ISP\r\n"])
    incoming = deque()
    outgoing = bytearray()
    writes = []
    dlab = False
    drained = False
    pending_polls = 0
    reached_rom = False
    timers = [{}, {}]
    wire = "0" * 10 + "10001001000110000010000010000010000110100101000111000101111" + "0100000000"
    edges = deque((i * 2500, bit) for i, bit in enumerate(wire) if i and bit != wire[i-1])
    streaming = False
    sending = False
    background_test = False
    input_start = None
    tx_latch = 0
    clock = 0

    led_mask = sum(1 << pin for pin in (22, 21, 18, 23, 25, 16))
    gpio = {0: 0, 0x10: 0, 0x14: (1 << 14) | (1 << silent_pin), 0x20: 0, 0x30: 0, 0x34: 0}
    def gpio_read(uc, offset, size, data):
        assert size == 4 and offset in gpio
        return gpio[offset]

    def gpio_write(uc, offset, size, value, data):
        nonlocal tx_latch
        assert size == 4
        if sending and offset in (0, 0x18, 0x1c):
            if offset == 0:
                assert value in (0, 1 << tx_pin)
                gpio[0] = value
            else:
                assert value == 1 << tx_pin
                tx_latch = value if offset == 0x18 else 0
            high = bool(gpio[0] & tx_latch)
            if high != bool(gpio[0x14] & (1 << rx_pin)):
                gpio[0x14] ^= 1 << rx_pin
                timer = timers[tx_timer]
                assert timer[0x28] >> (3 * tx_channel) & 7 == (5 if high else 6)
                timer[0x2c + 4 * tx_channel] = clock
                timer[0] |= 1 << (4 + tx_channel)
        elif offset in (0x38, 0x3c):
            assert value & ~led_mask == 0
            if offset == 0x38:
                gpio[0x34] |= value
            else:
                gpio[0x34] &= ~value
        else:
            assert (offset, value) in ((0, 0), (0x10, 0), (0x20, led_mask), (0x20, 0), (0x30, 0))
            gpio[offset] = value

    cpu.mmio_map(0x3fffc000, 0x1000, gpio_read, None, gpio_write, None)
    cpu.mem_map(0xe0014000, 0x1000)

    def timer_write(uc, offset, size, value, index):
        nonlocal streaming, sending, clock, edges
        regs = timers[index]
        assert size == 4 and offset in (0, 4, 0x0c, 0x14, 0x28, 0x70)
        if offset == 0:
            regs[0] = regs.get(0, 0) & ~value
            return
        regs[offset] = value
        if offset == 4 and value == 2 and index == 0:
            streaming = sending = False
        if offset == 4 and value == 1:
            assert regs[0x0c] == 47 and regs[0x14] == 0 and regs[0x70] == 0
            pins = sum(2 << (2 * pin) for pin in (2, 4, 6, 10, 11))
            assert uc.mem_read(0xE002C000, 4) == struct.pack("<I", pins | 5)
            expected_ccr = (0x16d, 0x2d)[index]
            if index == silent_timer:
                expected_ccr += 1 << (3 * silent_channel)  # starts dominant: arm falling
            if regs[0x28] == expected_ccr:
                if index == 0:
                    sending = outgoing.endswith(b"OK TX\r\n")
                    streaming = sending or outgoing.endswith(b"OK FRAMES\r\n")
                    edges = deque((i * 2500, b) for i, b in enumerate(wire) if i and b != wire[i-1])
                    clock = 0
                regs[8] = regs[0] = 0
                return
            assert regs[0x28] == (0x1ff, 0x3f)[index]
            regs[8] = 1_000_000 + index
            regs[0] = (0x60, 0x10)[index]
            for channel in range(3):
                regs[0x2c + channel * 4] = 1 + 16 * index + channel

    def timer_read(uc, offset, size, index):
        nonlocal clock, streaming, background_test, input_start
        assert size == 4 and offset in (0, 8, 0x28, 0x2c, 0x30, 0x34)
        if (streaming or background_test or input_start is not None) and offset == 8:
            if index == 0:
                clock += 100 if sending else 200
                if background_test or input_start is not None:
                    uc.mem_write(0xe0014008, struct.pack("<I", clock))
                if input_start is not None:
                    elapsed = clock - input_start
                    # A short bounce followed by a held press with the DIP flipped.
                    pressed = 2_000 < elapsed < 6_000 or 30_000 < elapsed < 140_000
                    gpio[0x14] = (gpio[0x14] | (1 << 7)) if elapsed > 30_000 else gpio[0x14]
                    if pressed:
                        gpio[0x14] &= ~(1 << 14)
                    else:
                        gpio[0x14] |= 1 << 14
                    if elapsed > 210_000:
                        input_start = None
                        incoming.extend(b"A\nPANEL?\n")
                if not sending and edges and edges[0][0] <= clock:
                    stamp, bit = edges.popleft()
                    gpio[0x14] = (gpio[0x14] & ~(1 << 4)) | (int(bit) << 4)
                    assert timers[0][0x28] >> 3 & 7 == (5 if bit == "1" else 6)
                    timers[0][0x30] = stamp
                    timers[0][0] |= 0x20
                if background_test and clock > 240_000:
                    background_test = False
                    incoming.extend(b"A\nPANEL?\n")
                elif streaming and clock > (240_000 if sending else (len(wire) + 2) * 2500):
                    streaming = False
                    incoming.append(10)
            return clock
        return timers[index][offset]

    for index, timer in enumerate((0xe0004000, 0xe0008000)):
        cpu.mmio_map(timer, 0x1000, timer_read, index, timer_write, index)

    def uart_read(uc, offset, size, user_data):
        nonlocal drained, pending_polls
        if offset == 0x14:
            if pending_polls:
                pending_polls -= 1
            else:
                drained = True
            return 0x20 | (0x40 if drained else 0) | bool(incoming)
        assert offset == 0 and not dlab
        return incoming.popleft()

    def uart_write(uc, offset, size, value, user_data):
        nonlocal dlab, drained, pending_polls, background_test, input_start
        writes.append((offset, value))
        if offset == 0x0C:
            dlab = bool(value & 0x80)
        elif offset == 0x20:
            assert value == 0x305 and drained and not incoming
            assert uc.mem_read(0xE01FC080, 4) == struct.pack("<I", 3)
            command = frames.popleft()
            if command is None:
                background_test = True
            elif command == "inputs":
                input_start = clock
            else:
                incoming.extend(command)
        elif offset == 0 and not dlab:
            outgoing.append(value)
            drained = False
            pending_polls = 4

    def at_rom(uc, address, size, user_data):
        nonlocal reached_rom
        assert drained
        assert not frames
        assert uc.mem_read(0xE01FC080, 4) == bytes(4)
        passive, active = bytes(outgoing).split(b"OK TX\r\n")
        assert passive == (b"TEKMAR-482-DEV\r\n"
                            b"PANEL 00000001 00000001 00000000 00000000 00000001 00000001 00000000\r\n"
                            b"PANEL 00000001 00000000 00000001 00000001 00000001 00000001 00000001\r\nOK LED\r\n"
                            b"PANEL 00000001 00000000 00000001 00000001 00000001 00000001 00000001\r\n"
                            b"ERR RX-INACTIVE\r\nOK RX\r\nOK RX\r\n"
                            b"RX 000f4240 000f4241 0000000d 00000002 00000000"
                            b" 00000003 00000011 00000000\r\nOK FRAMES\r\n"
                            b"F 0 000061a8 0e2f A 12300001a5\r\nEND 00000000\r\n")
        records = active.splitlines()
        assert records[0].split() == [b'F', str(tx_port).encode(), b'00009c40', b'0e2f', b'N', tx_body.hex().encode()], records
        assert records[1] == f'T {tx_port} NOACK'.encode()
        assert len(records) == 10 and all(row.startswith(b'D ') for row in records[2:7])
        assert records[2 + silent_port].split() == [b'D', str(silent_port).encode()] + [b'00000000'] * 12 + [b'00000003']
        assert records[-3:] == [b'END 00000000', b'OK RX', b'OK ISP']
        assert gpio[0] == tx_latch == 0
        assert all(t[0] == t[4] == t[0x28] == 0 for t in timers)
        assert gpio[0x34] & led_mask == led_mask
        assert gpio[0x20] == 0
        assert uc.mem_read(0xE01FC1A0, 4) == bytes(4)
        assert uc.mem_read(0xE01FC0C4, 4) == struct.pack("<I", 8)
        assert uc.reg_read(UC_ARM_REG_CPSR) & 0xE0 == 0xE0  # Thumb, IRQ/FIQ masked
        for reg in (UC_ARM_REG_R0, UC_ARM_REG_R1):
            assert 0x40000200 <= uc.reg_read(reg) <= 0x40007EE0 - 20
        command = uc.mem_read(uc.reg_read(UC_ARM_REG_R0), 4)
        assert struct.unpack("<I", command) == (57,)
        reached_rom = True
        uc.emu_stop()

    cpu.mmio_map(0xE000C000, 0x1000, uart_read, None, uart_write, None)
    cpu.hook_add(UC_HOOK_CODE, at_rom, begin=0x7FFFFFF0, end=0x7FFFFFF0)
    cpu.emu_start(base, 0xFFFFFFFF, count=3_000_000)
    assert reached_rom, "firmware did not hand off to ROM"
    assert (0x28, 0x10) in writes  # fractional divider disabled during measurement
    assert cpu.mem_read(0xE002C000, 4) == struct.pack("<I", 5)  # UART0 pins only


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--ram", action="store_true")
    parser.add_argument("image", type=Path)
    args = parser.parse_args()
    image = args.image.read_bytes()
    for port in range(5):
        smoke(image, args.ram, port)
    print("ARM image: panel, passive RX, independent TX/echo on all five ports, and ROM ISP handoff")
