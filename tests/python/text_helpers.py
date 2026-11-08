"""What the tests of the tools' texts share."""


def content(text: str) -> str:
    """A page's text without its closing line of facts (``[next_offset=2, ...]``)."""
    body = text.removesuffix("\n")
    start = body.rfind("\n") + 1
    last = body[start:]
    return text[:start] if last.startswith("[") and last.endswith("]") else text
