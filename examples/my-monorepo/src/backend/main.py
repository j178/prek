"""Backend main module."""

from utils import helper_function


def backend_main():
    """Backend main function."""
    result = helper_function()
    print(f"Backend result: {result}")
    return result


if __name__ == "__main__":
    backend_main()
