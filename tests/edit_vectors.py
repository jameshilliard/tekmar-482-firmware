"""Compare positional host block edits with frames emitted by the firmware state machine."""
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
from payloads.layouts import BLOCK_SELECTORS
from payloads.writes import BlockPatch, WritableBlock


def vector(block, old, new):
    mask = BLOCK_SELECTORS.get(block, (None, 0))[1]
    selector = old[0] & mask
    support = (1 << len(old)) - 1
    block = int(block).to_bytes(2, "little")
    key = b"\x42\x07" + block + bytes([mask, selector, support, len(old)]) + old + new
    query = bytes.fromhex("6d70000b") + block + bytes.fromhex("4240000000") + selector.to_bytes(4, "little")
    body = block + bytes([support]) + old
    report = (0x6e7421c0 | len(body)).to_bytes(4, "big") + body
    body = bytes.fromhex("4207") + block + new
    update = (0x6df00000 | len(body)).to_bytes(4, "big") + body
    op = "noop" if old == new[:len(old)] else "edit"
    return " ".join([op, *(x.hex() for x in (key, query, report, update))])


def verify(rows):
    subprocess.run(["cargo", "run", "--quiet", "--locked", "--example", "check_block_edits"],
                   cwd=ROOT, input="\n".join(rows), text=True, check=True)


def catalog_vectors():
    for block in WritableBlock:
        for field in block.fields:
            count = 0
            for length in range(1, 9):
                old = bytearray([1] * length)
                if block == WritableBlock.SCHEDULE_SETUP and length == 6:
                    old[-1] = 0
                for value in (0, 1, 2):
                    try:
                        new = BlockPatch.from_fields(block, {field: value}).apply((1 << length) - 1, bytes(old))
                    except ValueError:
                        continue
                    if new[:length] != old:
                        yield vector(block, bytes(old), new)
                        count += 1
                        break
            assert count, (block, field)


if __name__ == "__main__":
    verify(catalog_vectors())
