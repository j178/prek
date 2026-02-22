import os
import shutil
import subprocess
import sys


def main() -> None:
    # With `python -m prek`, runpy sets sys.argv[0] to the module path and argv[1:] to the args
    prek_args = sys.argv[1:]

    # Prefer prek next to the Python executable so that `python -m prek` uses the
    # same prek as that Python environment (e.g. .venv/bin/prek with .venv/bin/python)
    python_dir = os.path.dirname(os.path.abspath(sys.executable))
    prek_bin = os.path.join(python_dir, "prek")
    if sys.platform == "win32":
        prek_bin += ".exe"

    if not os.path.isfile(prek_bin):
        prek_bin = shutil.which("prek")
    if prek_bin is None:
        print("prek: command not found", file=sys.stderr)
        sys.exit(127)

    sys.exit(subprocess.run([prek_bin] + prek_args).returncode)


if __name__ == "__main__":
    main()
