import os
import shutil
import subprocess
import sys

from ._find_prek import find_prek_bin

def _run() -> None:
    prek = find_prek_bin()

    if sys.platform == "win32":
        import subprocess

        # Avoid emitting a traceback on interrupt
        try:
            completed_process = subprocess.run([prek, *sys.argv[1:]], env=env)
        except KeyboardInterrupt:
            sys.exit(2)

        sys.exit(completed_process.returncode)
    else:
        os.execvp(prek, [prek, *sys.argv[1:]])


if __name__ == "__main__":
    _run()
