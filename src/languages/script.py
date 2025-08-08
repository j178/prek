import sys
import re
from re import Pattern
from concurrent.futures import ThreadPoolExecutor


output = sys.stdout.buffer


def process_file(
    filename: str, pattern: Pattern[bytes], multiline: bool, negate: bool
) -> int:
    if multiline:
        if negate:
            return _process_filename_at_once_negated(pattern, filename)
        else:
            return _process_filename_at_once(pattern, filename)
    else:
        if negate:
            return _process_filename_by_line_negated(pattern, filename)
        else:
            return _process_filename_by_line(pattern, filename)


def _process_filename_by_line(pattern: Pattern[bytes], filename: str) -> int:
    retv = 0
    with open(filename, "rb") as f:
        for line_no, line in enumerate(f, start=1):
            if pattern.search(line):
                retv = 1
                output.write(f"{filename}:{line_no}:".encode())
                output.write(line.rstrip(b"\r\n"))
                output.write(b"\n")
    return retv


def _process_filename_at_once(pattern: Pattern[bytes], filename: str) -> int:
    retv = 0
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
    return retv


def _process_filename_by_line_negated(
    pattern: Pattern[bytes],
    filename: str,
) -> int:
    with open(filename, "rb") as f:
        for line in f:
            if pattern.search(line):
                return 0
        else:
            output.write(filename.encode())
            output.write(b"\n")
            return 1


def _process_filename_at_once_negated(
    pattern: Pattern[bytes],
    filename: str,
) -> int:
    with open(filename, "rb") as f:
        contents = f.read()
    match = pattern.search(contents)
    if match:
        return 0
    else:
        output.write(filename.encode())
        output.write(b"\n")
        return 1


def main():
    ignore_case = sys.argv[1] == "1"
    multiline = sys.argv[2] == "1"
    negate = sys.argv[3] == "1"
    concurrency = int(sys.argv[4])
    pattern = sys.argv[5].encode()

    flags = re.IGNORECASE if ignore_case else 0
    if multiline:
        flags |= re.MULTILINE | re.DOTALL

    pattern = re.compile(pattern, flags)

    pool = ThreadPoolExecutor(max_workers=concurrency)
    futures = []

    for filename in sys.stdin.readlines():
        filename = filename.strip()
        futures.append(pool.submit(process_file, filename, pattern, multiline, negate))

    pool.shutdown(wait=True)

    ret = 0
    for future in futures:
        ret |= future.result()

    sys.exit(ret)


if __name__ == "__main__":
    main()
