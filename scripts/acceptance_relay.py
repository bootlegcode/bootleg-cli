#!/usr/bin/env python3
"""Linux-only acceptance relay: bind the owned bridge, never a wildcard interface.

Only peers in that bridge subnet may connect. The destination is fixed loopback;
no proxy headers, target URLs, credentials or payloads are logged.
"""
import asyncio
import ipaddress
import sys


async def serve(gateway, subnet, port):
    network = ipaddress.IPv4Network(subnet)
    address = ipaddress.IPv4Address(gateway)
    if (not address.is_private or address.is_loopback or address.is_unspecified
            or address.is_multicast or address.is_reserved or address not in network):
        raise ValueError("relay requires an owned private bridge address")

    async def connection(reader, writer):
        upstream = None
        try:
            peer = ipaddress.IPv4Address(writer.get_extra_info("peername")[0])
            if peer not in network:
                return
            source, upstream = await asyncio.open_connection("127.0.0.1", port)

            async def copy(source, target):
                try:
                    while data := await source.read(65536):
                        target.write(data)
                        await target.drain()
                finally:
                    target.close()

            await asyncio.gather(copy(reader, upstream), copy(source, writer))
        except (OSError, ConnectionError):
            pass
        finally:
            writer.close()
            if upstream:
                upstream.close()

    server = await asyncio.start_server(connection, gateway, port)
    async with server:
        await server.serve_forever()


if __name__ == "__main__":
    asyncio.run(serve(sys.argv[1], sys.argv[2], int(sys.argv[3])))
