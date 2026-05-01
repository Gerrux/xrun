"""Async wrappers around the `xrun` CLI binary.

Keeps subprocess plumbing isolated from the UI layer.
"""
from __future__ import annotations

import asyncio
import json
import re
import sys
from pathlib import Path
from typing import Any, Iterable


async def _run(*args: str, timeout: int = 30) -> tuple[int, str, str]:
    kwargs: dict[str, Any] = dict(
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    if sys.platform == "win32":
        import subprocess as _sub
        kwargs["creationflags"] = _sub.CREATE_NO_WINDOW
    try:
        proc = await asyncio.create_subprocess_exec("xrun", *args, **kwargs)
        out, err = await asyncio.wait_for(proc.communicate(), timeout=timeout)
        return (
            proc.returncode or 0,
            out.decode(errors="replace"),
            err.decode(errors="replace"),
        )
    except asyncio.TimeoutError:
        return -1, "", "timeout"
    except FileNotFoundError:
        return -1, "", "xrun not found in PATH"


# ── Mutations ─────────────────────────────────────────────────────────────────

async def stop_run(run_id: str, force: bool = False) -> tuple[bool, str]:
    args = ["stop", run_id]
    if force:
        args.append("--force")
    code, _, err = await _run(*args)
    return code == 0, err.strip()


async def rerun_run(run_id: str) -> tuple[bool, str]:
    code, out, err = await _run("rerun", run_id)
    return code == 0, (out + err).strip()


async def rerun_with_patches(
    run_id: str, patches: dict[str, str]
) -> tuple[bool, str]:
    """Rerun a run with patched args.

    Patches are passed as ``--patch key=value`` pairs, mirroring
    ``xrun rerun --patch run.args.KEY=VALUE``.  Keys are prefixed with
    ``run.args.`` so the CLI can locate them inside the manifest.
    """
    args = ["rerun", run_id]
    for key, val in patches.items():
        args += ["--patch", f"run.args.{key}={val}"]
    code, out, err = await _run(*args, timeout=120)
    return code == 0, (out + err).strip()


async def fix_status(run_id: str | None = None) -> tuple[bool, str]:
    """Reconcile stale 'running' runs against the vendor.

    Pass a run id to target one run; pass None to scan all currently-running
    runs. The CLI returns 0 even when nothing changed, so the message is the
    primary signal back to the user.
    """
    args = ["fix-status"]
    if run_id:
        args.append(run_id)
    code, out, err = await _run(*args, timeout=60)
    msg = (out + err).strip()
    return code == 0, msg


async def launch(
    manifest: str,
    dry_run: bool = False,
    name: str | None = None,
    detach: bool = True,
) -> tuple[bool, str]:
    args = ["launch", manifest, "--json"]
    if dry_run:
        args.append("--dry-run")
    if detach and not dry_run:
        args.append("--detach")
    if name:
        args += ["--name", name]
    code, out, err = await _run(*args, timeout=120)
    return code == 0, (out + err).strip()


async def pull(
    run_id: str,
    ckpt: str = "latest",
    artifacts: bool = False,
    into: str | None = None,
) -> tuple[bool, str]:
    args = ["pull", run_id, "--ckpt", ckpt]
    if artifacts:
        args.append("--artifacts")
    if into:
        args += ["--into", into]
    code, out, err = await _run(*args, timeout=600)
    return code == 0, (out + err).strip()


# ── Reads ─────────────────────────────────────────────────────────────────────

async def config_show() -> tuple[bool, dict[str, Any], str]:
    code, out, err = await _run("config", "show", "--json", timeout=10)
    if not out:
        return False, {}, err.strip()
    try:
        return True, json.loads(out), ""
    except Exception:
        # Tolerate plain key=value output as well
        return False, {}, f"parse error: {out[:200]}"


async def doctor() -> tuple[bool, dict[str, Any], str]:
    code, out, err = await _run("doctor", "--json", timeout=10)
    if code != 0 and not out:
        return False, {}, err.strip()
    try:
        return True, json.loads(out), ""
    except Exception as exc:
        return False, {}, f"parse error: {exc}\n{out[:200]}"


async def metrics(run_id: str, key: str | None = None) -> tuple[bool, Any, str]:
    args = ["metrics", run_id, "--json"]
    if key:
        args += ["--key", key]
    code, out, err = await _run(*args, timeout=15)
    if code != 0 and not out:
        return False, None, err.strip()
    try:
        return True, json.loads(out), ""
    except Exception as exc:
        return False, None, f"parse error: {exc}"


async def list_artifacts(run_id: str, path: str = "") -> tuple[bool, list[dict], str]:
    # xrun pull / artifact listing is not yet implemented in the CLI.
    # Return empty so the screen shows the "not available" state rather than a clap error.
    return True, [], ""


async def get_logs(run_id: str, last_n: int = 500) -> str:
    _, out, err = await _run("logs", run_id, timeout=15)
    if not out and err:
        return f"[#f7768e]error:[/] {err}"
    lines = out.splitlines()
    if not lines:
        return (
            "[#414868]No local log snapshot yet.[/]\n\n"
            "[#565f89]Stream live output with:[/]\n"
            f"[bold #7aa2f7]  xrun logs -f {run_id}[/]\n\n"
            "[#414868]The poller writes a local snapshot every ~5 s once the run is running.[/]"
        )
    if len(lines) > last_n:
        lines = [
            f"[#414868]… ({len(lines) - last_n} earlier lines omitted) …[/]",
            *lines[-last_n:],
        ]
    return "\n".join(lines)


def read_manifest(path: str) -> str:
    if not path:
        return "# manifest path unknown"
    try:
        return Path(path).read_text(encoding="utf-8")
    except OSError as e:
        return f"# cannot read manifest: {e}"


# Directories never worth descending into when looking for manifests.
_SKIP_DIRS: frozenset[str] = frozenset({
    "node_modules", "target", "venv", "__pycache__",
    "dist", "build", "site-packages", "egg-info",
})

# Conventional roots, tried in order if no explicit `exp_dir` is configured.
_DEFAULT_ROOTS: tuple[str, ...] = ("exp", "experiments", "manifests")

# Max directory depth (relative to each root) to keep walks bounded.
_MAX_DEPTH = 6

# Cap parsed bytes per file — manifests are small; ignore huge YAML blobs early.
_SNIFF_BYTES = 8 * 1024

# Top-level keys required by the xrun manifest schema
# (crates/xrun-core/src/manifest/types.rs::Manifest).
_REQUIRED_KEYS = ("name", "vendor", "run")
_KEY_RE = {
    k: re.compile(rf"(?m)^{k}\s*:")
    for k in _REQUIRED_KEYS
}


def _looks_like_manifest(path: Path) -> bool:
    """Cheap content sniff: top-level mapping must contain name + vendor + run.

    Avoids a YAML dependency by line-anchoring the keys (no leading whitespace),
    which excludes nested children. False positives are possible only for files
    that genuinely declare all three keys at the top level — which IS the
    manifest schema.
    """
    try:
        with path.open("rb") as fh:
            head = fh.read(_SNIFF_BYTES)
    except OSError:
        return False
    try:
        text = head.decode("utf-8", errors="replace")
    except Exception:
        return False
    return all(rx.search(text) is not None for rx in _KEY_RE.values())


def _walk(root: Path, max_depth: int) -> Iterable[Path]:
    """Yield .yaml/.yml files under `root`, skipping noisy/hidden dirs."""
    root = root.resolve()
    stack: list[tuple[Path, int]] = [(root, 0)]
    while stack:
        d, depth = stack.pop()
        try:
            with __import__("os").scandir(d) as it:
                for entry in it:
                    name = entry.name
                    if name.startswith("."):
                        continue
                    try:
                        if entry.is_dir(follow_symlinks=False):
                            if name.lower() in _SKIP_DIRS:
                                continue
                            if depth + 1 <= max_depth:
                                stack.append((Path(entry.path), depth + 1))
                        elif entry.is_file(follow_symlinks=False):
                            lower = name.lower()
                            if lower.endswith(".yaml") or lower.endswith(".yml"):
                                yield Path(entry.path)
                    except OSError:
                        continue
        except OSError:
            continue


def discover_manifests(
    exp_dir: str | None = None,
    base: Path | None = None,
    extra_roots: list[str] | None = None,
) -> list[Path]:
    """Find xrun manifests under conventional experiment directories.

    Strategy:
      1. If `exp_dir` is given (typically from `defaults.exp_dir`), scan only it.
      2. Otherwise scan whichever of `exp/`, `experiments/`, `manifests/` exist.
      3. Never walk cwd recursively — that picks up CI configs, lockfiles, etc.
      4. Within a root: skip hidden + build dirs, cap depth, and accept only
         files whose top-level YAML keys match the manifest schema.
    """
    base = (base or Path.cwd()).resolve()

    if exp_dir:
        roots = [base / exp_dir]
    else:
        roots = [base / r for r in _DEFAULT_ROOTS]
    if extra_roots:
        roots.extend(base / r for r in extra_roots)

    found: list[Path] = []
    seen: set[Path] = set()
    for root in roots:
        if not root.exists() or not root.is_dir():
            continue
        for p in _walk(root, _MAX_DEPTH):
            rp = p.resolve()
            if rp in seen:
                continue
            seen.add(rp)
            if not _looks_like_manifest(p):
                continue
            found.append(p)

    found.sort(
        key=lambda p: (p.stat().st_mtime if p.exists() else 0),
        reverse=True,
    )
    return found[:200]
