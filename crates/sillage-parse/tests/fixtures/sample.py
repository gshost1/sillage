"""Module docstring."""
import os
from collections import defaultdict

COUNTER = 0


def top_level(a, b=2, *args, c: int = 3, **kwargs):
    # comment
    total = a + b + c
    for x, y in zip(args, args[1:]):
        total += x * y
    return total


class Repo:
    kind = "repo"

    def __init__(self, path):
        self.path = path
        self.files = defaultdict(list)

    @property
    def name(self):
        return os.path.basename(self.path)

    def scan(self, exts=("py", "rs")):
        global COUNTER
        seen = set()
        try:
            with open(self.path) as fh, open(os.devnull, "w") as sink:
                for line in fh:
                    if (n := len(line)) > 80:
                        print(f"long line {n}: {line!r}", file=sink)
                    seen.add(line.strip())
        except (IOError, OSError) as err:
            print("error", err)
        finally:
            COUNTER += 1
        squares = [v * v for v in range(10) if v % 2]
        lookup = {k: v for k, v in enumerate(squares)}
        key = lambda item: item[1]
        return sorted(lookup.items(), key=key), seen

    async def fetch(self, client, *, retries=3):
        count = 0

        def bump():
            nonlocal count
            count += 1
            return count

        while retries > 0:
            retries -= 1
            bump()
            result = await client.get(self.path)
            if result is None:
                continue
            elif result.ok:
                break
            else:
                raise ValueError("bad")
        a, (b, *rest) = 1, (2, 3, 4)
        data = b"bytes" + rb"raw"
        return result, a, b, rest, data, True, None, 3.14, 0x1F
