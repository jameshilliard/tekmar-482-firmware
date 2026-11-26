# /// script
# requires-python = ">=3.10"
# dependencies = ["unicorn==2.1.4"]
# ///
"""Execute the built ARM image; model only UART status and the ROM entry boundary."""

import argparse
import struct
from collections import deque
from pathlib import Path

from unicorn import UC_ARCH_ARM, UC_HOOK_CODE, UC_MODE_ARM, UC_PROT_EXEC, UC_PROT_READ, Uc
from unicorn.arm_const import UC_ARM_REG_CPSR, UC_ARM_REG_R0, UC_ARM_REG_R1


def smoke(image: bytes, ram: bool) -> None:
    cpu = Uc(UC_ARCH_ARM, UC_MODE_ARM)
    if not ram:
        cpu.mem_map(0, 0x10000, UC_PROT_EXEC | UC_PROT_READ)
    cpu.mem_map(0x40000000, 0x8000)
    base = 0x40001000 if ram else 0
    cpu.mem_write(base, image)
    cpu.mem_map(0x7FFFF000, 0x1000)

    # Leave all tN4 GPIO and timer registers unmapped: touching them fails the test.
    cpu.mem_map(0xE01FC000, 0x1000)
    cpu.mem_map(0xE002C000, 0x1000)
    cpu.mem_write(0xE01FC088, struct.pack("<I", 1 << 10))  # PLL0 locked
    frames = deque([b"?", b"A!", b"A\nBAD-ENTER-ISP\n", b"A\nID?\n", b"A\nENTER-ISP\r\n"])
    incoming = deque()
    outgoing = bytearray()
    writes = []
    dlab = False
    drained = False
    pending_polls = 0
    reached_rom = False

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
        nonlocal dlab, drained, pending_polls
        writes.append((offset, value))
        if offset == 0x0C:
            dlab = bool(value & 0x80)
        elif offset == 0x20:
            assert value == 0x305 and drained and not incoming
            assert uc.mem_read(0xE01FC080, 4) == struct.pack("<I", 3)
            incoming.extend(frames.popleft())
        elif offset == 0 and not dlab:
            outgoing.append(value)
            drained = False
            pending_polls = 4

    def at_rom(uc, address, size, user_data):
        nonlocal reached_rom
        assert drained
        assert not frames
        assert uc.mem_read(0xE01FC080, 4) == bytes(4)
        assert outgoing == b"TEKMAR-482-DEV\r\nOK ISP\r\n"
        assert uc.reg_read(UC_ARM_REG_CPSR) & 0xE0 == 0xE0  # Thumb, IRQ/FIQ masked
        for reg in (UC_ARM_REG_R0, UC_ARM_REG_R1):
            assert 0x40000200 <= uc.reg_read(reg) <= 0x40007EE0 - 20
        command = uc.mem_read(uc.reg_read(UC_ARM_REG_R0), 4)
        assert struct.unpack("<I", command) == (57,)
        reached_rom = True
        uc.emu_stop()

    cpu.mmio_map(0xE000C000, 0x1000, uart_read, None, uart_write, None)
    cpu.hook_add(UC_HOOK_CODE, at_rom, begin=0x7FFFFFF0, end=0x7FFFFFF0)
    cpu.emu_start(base, 0xFFFFFFFF, count=100_000)
    assert reached_rom, "firmware did not hand off to ROM"
    assert (0x28, 0x10) in writes  # fractional divider disabled during measurement
    assert cpu.mem_read(0xE002C000, 4) == struct.pack("<I", 5)  # UART0 pins only
    print("ARM image: autobaud training -> identify -> ACK drained -> crystal -> ROM call 57")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--ram", action="store_true")
    parser.add_argument("image", type=Path)
    args = parser.parse_args()
    smoke(args.image.read_bytes(), args.ram)
