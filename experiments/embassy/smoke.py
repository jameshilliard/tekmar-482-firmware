# /// script
# requires-python = ">=3.10"
# dependencies = ["unicorn==2.1.4"]
# ///
"""Execute the trial against a 1 kHz counter and confirm timed wakes and ISP return."""

import struct
import sys
from pathlib import Path

from unicorn import UC_ARCH_ARM, UC_HOOK_CODE, UC_MODE_ARM, Uc
from unicorn.arm_const import UC_ARM_REG_R0


def run(image, vpbd):
    cpu = Uc(UC_ARCH_ARM, UC_MODE_ARM)
    cpu.mem_map(0x40000000, 0x8000)
    cpu.mem_write(0x40001000, image)
    cpu.mem_map(0x7ffff000, 0x1000)
    cpu.mem_map(0xe01fc000, 0x1000)
    cpu.mem_write(0xe01fc100, struct.pack("<I", vpbd))
    timer = {}
    received = []
    tick = 0
    returned = False

    def timer_write(uc, offset, size, value, data):
        assert offset in (4, 0x0c, 0x14, 0x70)
        timer[offset] = value

    def timer_read(uc, offset, size, data):
        nonlocal tick
        assert offset == 8 and timer[4] == 1 and timer[0x14] == timer[0x70] == 0
        assert timer[0x0c] == 12_000 // (4, 1, 2)[vpbd] - 1
        tick += 1
        return tick

    def uart_read(uc, offset, size, data):
        assert offset == 0x14
        return 0x60

    def uart_write(uc, offset, size, value, data):
        assert offset == 0
        received.append((tick, value))

    def rom(uc, addr, size, data):
        nonlocal returned
        table = uc.reg_read(UC_ARM_REG_R0)
        assert uc.mem_read(table, 4) == struct.pack("<I", 57)
        assert uc.mem_read(0xe01fc0c4, 4) == bytes(4) and timer[4] == 0
        returned = True
        uc.emu_stop()

    cpu.mmio_map(0xe0004000, 0x1000, timer_read, None, timer_write, None)
    cpu.mmio_map(0xe000c000, 0x1000, uart_read, None, uart_write, None)
    cpu.hook_add(UC_HOOK_CODE, rom, begin=0x7ffffff0, end=0x7ffffff0)
    cpu.emu_start(0x40001000, 0xffffffff, count=1_000_000)
    assert returned and bytes(b for _, b in received) == b"FFSF\r\n", received
    for (tick, _), due in zip(received, (100, 200, 225, 300, 400, 400), strict=True):
        assert due <= tick <= due + 8, (tick, due)


if __name__ == "__main__":
    for divider in range(3):
        run(Path(sys.argv[1]).read_bytes(), divider)
    print("ARM image: two Embassy tasks wake at their deadlines and return to ROM ISP")
