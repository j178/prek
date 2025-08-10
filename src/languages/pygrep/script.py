import json
import io
import re
import sys
from concurrent.futures import ThreadPoolExecutor
from queue import Queue
from re import Pattern
from threading import Thread


def process_file(
    filename: str, pattern: Pattern[bytes], multiline: bool, negate: bool, queue: Queue
) -> None:
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
        queue.put((ret, output))
    except Exception as e:
        # Put error result in queue so consumer can handle it
        queue.put((1, f"Error processing {filename}: {e}\n".encode()))


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

    queue = Queue()
    pool = ThreadPoolExecutor(max_workers=concurrency)

    # Use a sentinel value to signal completion
    SENTINEL = (None, None)

    def producer():
        try:
            for line in sys.stdin:
                line = line.strip()
                if not line:
                    break
                pool.submit(
                    process_file, line.strip(), pattern, multiline, negate, queue
                )

            # Wait for all tasks to complete
            pool.shutdown(wait=True)
        finally:
            # Ensure sentinel is sent even if there's an error
            queue.put(SENTINEL)

    def consumer():
        retv = 0
        try:
            while True:
                ret, output = queue.get()

                # Check for sentinel value
                if ret is None and output is None:
                    queue.task_done()
                    break

                retv |= ret
                if output:
                    sys.stdout.buffer.write(output)
                    sys.stdout.buffer.flush()

                queue.task_done()
        except Exception:
            pass

        # Write final return code
        sys.stderr.buffer.write(f'{{"code": {retv}}}\n'.encode())
        sys.stderr.buffer.flush()

    t1 = Thread(target=producer)
    t2 = Thread(target=consumer)
    t1.start()
    t2.start()

    # Wait for both threads to complete
    t1.join()
    t2.join()


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
