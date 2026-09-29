from __future__ import annotations

import json
import io
import re
import sys
from re import Pattern


def process_file(
    filename: str, pattern: Pattern[bytes], multiline: bool, negate: bool
) -> tuple[int, bytes]:
    try:
        if multiline:
            if negate:
                ret, output = _process_filename_at_once_negated(pattern, filename)
            else:
                ret, output = _process_filename_at_once(pattern, filename)
        else:
            if negate:
                ret, output = _process_filename_by_line_negated(pattern, filename)
            else:
                ret, output = _process_filename_by_line(pattern, filename)
        return ret, output
    except Exception as e:
        return 1, f"Error processing {filename}: {e}\n".encode()


def _process_filename_by_line(
    pattern: Pattern[bytes], filename: str
) -> tuple[int, bytes]:
    retv = 0
    output = io.BytesIO()
    with open(filename, "rb") as f:
        for line_no, line in enumerate(f, start=1):
            if pattern.search(line):
                retv = 1
                output.write(f"{filename}:{line_no}:".encode())
                output.write(line.rstrip(b"\r\n"))
                output.write(b"\n")
    return retv, output.getvalue()


def _process_filename_at_once(
    pattern: Pattern[bytes], filename: str
) -> tuple[int, bytes]:
    retv = 0
    output = io.BytesIO()
    with open(filename, "rb") as f:
        contents = f.read()
        match = pattern.search(contents)
        if match:
            retv = 1
            line_no = contents[: match.start()].count(b"\n")
            output.write(f"{filename}:{line_no + 1}:".encode())

            matched_lines = match[0].split(b"\n")
            matched_lines[0] = contents.split(b"\n")[line_no]

            output.write(b"\n".join(matched_lines))
            output.write(b"\n")
    return retv, output.getvalue()


def _process_filename_by_line_negated(
    pattern: Pattern[bytes], filename: str
) -> tuple[int, bytes]:
    with open(filename, "rb") as f:
        for line in f:
            if pattern.search(line):
                return 0, b""
        else:
            return 1, filename.encode() + b"\n"


def _process_filename_at_once_negated(
    pattern: Pattern[bytes], filename: str
) -> tuple[int, bytes]:
    with open(filename, "rb") as f:
        contents = f.read()
    match = pattern.search(contents)
    if match:
        return 0, b""
    else:
        return 1, filename.encode() + b"\n"


def run(
    ignore_case: bool, multiline: bool, negate: bool, concurrency: int, pattern: bytes
):
    flags = re.IGNORECASE if ignore_case else 0
    if multiline:
        flags |= re.MULTILINE | re.DOTALL
    pattern = re.compile(pattern, flags)

    filenames = []
    for line in sys.stdin:
        filename = line.strip()
        if not filename:
            break
        filenames.append(filename)

    def check(filename):
        return process_file(filename, pattern, multiline, negate)

    def results():
        # Keep single-file runs on the main thread to avoid worker startup.
        if len(filenames) <= 1:
            yield from map(check, filenames)
            return

        from concurrent.futures import ThreadPoolExecutor

        with ThreadPoolExecutor(max_workers=concurrency) as pool:
            yield from pool.map(check, filenames)

    retv = 0
    for ret, output in results():
        retv |= ret
        sys.stdout.buffer.write(output)

    sys.stderr.buffer.write(f'{{"code": {retv}}}\n'.encode())


def main():
    ignore_case = sys.argv[1] == "1"
    multiline = sys.argv[2] == "1"
    negate = sys.argv[3] == "1"
    concurrency = int(sys.argv[4])
    pattern = sys.argv[5].encode()

    try:
        run(ignore_case, multiline, negate, concurrency, pattern)
    except re.error as e:
        error = {"type": "Regex", "message": str(e)}
        sys.stderr.buffer.write(json.dumps(error).encode())
        sys.stderr.flush()
        sys.exit(1)
    except OSError as e:
        error = {"type": "IO", "message": str(e)}
        sys.stderr.buffer.write(json.dumps(error).encode())
        sys.stderr.flush()
        sys.exit(1)
    except Exception as e:
        error = {"type": "Unknown", "message": repr(e)}
        sys.stderr.buffer.write(json.dumps(error).encode())
        sys.stderr.flush()
        sys.exit(1)


if __name__ == "__main__":
    main()
