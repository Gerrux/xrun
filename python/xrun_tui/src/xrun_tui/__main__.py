from __future__ import annotations

import argparse
import atexit
import sys

from xrun_tui.app import XrunApp


def _restore_terminal() -> None:
    # Defensive cleanup for cases where Textual's driver doesn't get to run
    # `stop_application_mode` (Ctrl+C during shutdown, terminal window closed
    # mid-frame, exception in writer thread). Without this, mouse-tracking
    # (?1003) and focus-tracking (?1004) stay enabled in the parent shell —
    # the user then sees raw `\x1b[<…M` and `\x1b[I/O` printed on every
    # mouse move or window-focus change.
    try:
        out = sys.__stdout__
        if out is None or not out.isatty():
            return
        out.write(
            "\x1b[?1000l"   # X10/VT200 mouse off
            "\x1b[?1002l"   # button-event mouse off
            "\x1b[?1003l"   # any-event mouse off
            "\x1b[?1006l"   # SGR mouse off
            "\x1b[?1015l"   # urxvt mouse off
            "\x1b[?1004l"   # focus tracking off
            "\x1b[?2004l"   # bracketed paste off
            "\x1b[<u"       # kitty keyboard protocol pop
            "\x1b[?1049l"   # leave alt screen
            "\x1b[?25h"     # show cursor
        )
        out.flush()
    except Exception:
        pass


def main() -> None:
    p = argparse.ArgumentParser(prog="xrun-tui")
    p.add_argument(
        "--wizard",
        action="store_true",
        help="Skip splash and open the first-run wizard immediately.",
    )
    args = p.parse_args()
    atexit.register(_restore_terminal)
    XrunApp(start_in_wizard=args.wizard).run()


if __name__ == "__main__":
    main()
