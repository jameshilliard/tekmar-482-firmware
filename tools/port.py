"""Bounded, exclusive asyncio access to the development gateway's local UART."""

import asyncio
from contextlib import asynccontextmanager

import serialx


class Link:
    def __init__(self, reader, writer):
        self.reader = reader
        self.writer = writer

    async def write(self, data):
        self.writer.write(data)
        await asyncio.wait_for(self.writer.drain(), 3)

    async def readline(self, limit=128, timeout=3):
        data = await asyncio.wait_for(self.reader.readuntil(b"\n"), timeout)
        if len(data) > limit:
            raise ValueError(f"oversized serial line: {len(data)} bytes")
        return data


@asynccontextmanager
async def connect(path, baudrate=9600):
    reader, writer = await serialx.open_serial_connection(path, baudrate, exclusive=True)
    try:
        yield Link(reader, writer)
    finally:
        writer.close()
        await asyncio.wait_for(writer.wait_closed(), 3)
