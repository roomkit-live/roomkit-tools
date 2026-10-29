"""Local HTTP responses for transport tests, never production networking."""

import asyncio
import json
from pathlib import Path

FIXTURE = json.loads((Path(__file__).resolve().parents[1] / "fixtures/search.json").read_text())


class SearchServer:
    def __init__(self):
        self.seen = []
        self.tasks = set()
        self.received = asyncio.Event()

    async def __aenter__(self):
        self.server = await asyncio.start_server(self.serve, "127.0.0.1", 0)
        self.origin = f"http://127.0.0.1:{self.server.sockets[0].getsockname()[1]}"
        return self

    async def __aexit__(self, *_):
        self.server.close()
        await self.server.wait_closed()
        for task in self.tasks:
            task.cancel()
        await asyncio.gather(*self.tasks, return_exceptions=True)

    async def serve(self, reader, writer):
        task = asyncio.current_task()
        self.tasks.add(task)
        try:
            request = await reader.readuntil(b"\r\n\r\n")
            path = request.split()[1].decode()
            headers = {}
            for line in request.decode().split("\r\n")[1:]:
                if ":" in line:
                    key, value = line.split(":", 1)
                    headers[key.lower()] = value.strip()
            body = await reader.readexactly(int(headers.get("content-length", 0)))
            self.seen.append({"path": path, "headers": headers, "body": json.loads(body)})
            self.received.set()
            route = FIXTURE["routes"][path]
            await asyncio.sleep(route.get("delay_ms", 0) / 1000)
            body = route.get("body", "")
            body = body.encode() if isinstance(body, str) else bytes(body)
            headers = {"Connection": "close", **route.get("headers", {})}
            if not route.get("no_length"):
                headers["Content-Length"] = str(len(body))
            if "type" in route:
                headers["Content-Type"] = route["type"]
            head = f"HTTP/1.1 {route.get('status', 200)} Fixture\r\n"
            head += "".join(f"{k}: {v}\r\n" for k, v in headers.items()) + "\r\n"
            writer.write(head.encode())
            await writer.drain()
            await asyncio.sleep(route.get("body_delay_ms", 0) / 1000)
            writer.write(body)
            await writer.drain()
        except (ConnectionError, asyncio.IncompleteReadError):
            pass
        finally:
            writer.close()
            try:
                await writer.wait_closed()
            except ConnectionError:
                pass
            self.tasks.discard(task)
