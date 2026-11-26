"""Bounded, exclusive asyncio access to the development gateway's local UART."""

import asyncio
from collections import deque

from frames import Decoder
from contextlib import asynccontextmanager

import serialx


class Link:
    def __init__(self, reader, writer, baudrate):
        self.baudrate = baudrate
        self.decoder = Decoder()
        self.packets = deque()
        self.reader = reader
        self.writer = writer

    async def packet(self):
        while not self.packets:
            chunk = await self.reader.read(256)
            if not chunk:
                raise EOFError("gateway closed")
            self.packets.extend(self.decoder.feed(chunk))
        return self.packets.popleft()

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
        yield Link(reader, writer, baudrate)
    finally:
        writer.close()
        await asyncio.wait_for(writer.wait_closed(), 3)
