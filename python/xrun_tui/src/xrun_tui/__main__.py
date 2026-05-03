from __future__ import annotations

import argparse

from xrun_tui.app import XrunApp


def main() -> None:
    p = argparse.ArgumentParser(prog="xrun-tui")
    p.add_argument(
        "--wizard",
        action="store_true",
        help="Skip splash and open the first-run wizard immediately.",
    )
    args = p.parse_args()
    XrunApp(start_in_wizard=args.wizard).run()


if __name__ == "__main__":
    main()
