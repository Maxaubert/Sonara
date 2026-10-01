"""The runtime copy: find the plugin tree to deploy from, read its version, and
copy its sonara package into the stable ~/.sonara/app the daemon runs."""
from __future__ import annotations

import json
import os
import shutil
import time
from typing import Optional

from sonara import install_record, paths


def read_plugin_version(plugin_root: str) -> str:
    """Return the plugin's declared version, or "" if unreadable.

    Reads <plugin_root>/.claude-plugin/plugin.json 'version'; falls back to the
    CLAUDE_PLUGIN_VERSION env var. Never raises (version is advisory).
    """
    path = os.path.join(plugin_root, ".claude-plugin", "plugin.json")
    try:
        with open(path, "r", encoding="utf-8") as f:
            data = json.load(f)
        v = data.get("version") if isinstance(data, dict) else None
        if isinstance(v, str) and v:
            return v
    except Exception:  # noqa: BLE001 - version is advisory, never fatal
        pass
    return os.environ.get("CLAUDE_PLUGIN_VERSION", "") or ""


def copy_app(plugin_root: str) -> str:
    """Copy the plugin's sonara package into the stable APP_DIR. Returns APP_DIR.

    Overwrites on every install so a plugin update fully refreshes the copy
    (stale modules from a prior version do not linger). The daemon's scheduled
    task points PYTHONPATH at APP_DIR, decoupling the long-lived daemon from the
    version-pinned marketplace cache.
    """
    app_dir = str(paths.APP_DIR)
    src_pkg = os.path.join(plugin_root, "src", "sonara")
    dst_pkg = os.path.join(app_dir, "sonara")
    new_pkg = dst_pkg + ".new"
    old_pkg = dst_pkg + ".old"
    os.makedirs(app_dir, exist_ok=True)
    # Crash-safe swap (#23): build the fresh copy NEXT TO the live one, then
    # rename it in. The old rmtree-then-copytree deleted the live app FIRST, so
    # any failure (classically: the running task's workdir locking a directory)
    # left a gutted install the respawn loop could not run. A failed copytree
    # now leaves the live app untouched.
    for stale in (new_pkg, old_pkg):                # prior-crash residue
        if os.path.isdir(stale):
            shutil.rmtree(stale, ignore_errors=True)
    # Repo bytecode is dead weight in the app and can mask a source change.
    shutil.copytree(src_pkg, new_pkg,
                    ignore=shutil.ignore_patterns("__pycache__"))
    if os.path.isdir(dst_pkg):
        rename_retrying(dst_pkg, old_pkg)
    try:
        rename_retrying(new_pkg, dst_pkg)
    except OSError:
        # #127: the old package is already aside. Put it back so a live
        # 'sonara' package always exists; the fresh copy stays as residue
        # that the next install sweeps.
        if os.path.isdir(old_pkg) and not os.path.isdir(dst_pkg):
            rename_retrying(old_pkg, dst_pkg)
        raise
    if os.path.isdir(old_pkg):
        shutil.rmtree(old_pkg, ignore_errors=True)  # best-effort; retried next install
    return app_dir


def rename_retrying(src: str, dst: str, attempts: int = 10,
                    delay: float = 0.3) -> None:
    """os.rename that retries a PermissionError. On Windows a just-written tree
    (antivirus, indexer) or one a just-exited daemon still pins is often denied
    for a moment, and a retry a little later succeeds (#127)."""
    for attempt in range(attempts):
        try:
            os.rename(src, dst)
            return
        except PermissionError:
            if attempt == attempts - 1:
                raise
            time.sleep(delay)


def is_plugin_root(path) -> bool:
    """True if *path* is a plugin checkout install() can deploy from: it has the
    package source, the hook entry the hooks point at, and the hooks file."""
    if not path:
        return False
    return (os.path.isfile(os.path.join(path, "src", "sonara", "__init__.py"))
            and os.path.isfile(os.path.join(path, "bin", "sonara-hook"))
            and os.path.isfile(os.path.join(path, "hooks", "hooks.json")))


def print_no_plugin_root() -> None:
    print("Cannot find the Sonara plugin files (src/sonara, bin/sonara-hook, "
          "hooks/hooks.json) next to this copy of Sonara, which looks like "
          "the deployed runtime in ~/.sonara. Nothing was changed. Run "
          "/sonara:install in Claude Code, or <plugin folder>/bin/sonara install.")


def resolve_plugin_root() -> Optional[str]:
    """The plugin tree install() deploys from, or None.

    repo_root() is right when the CLI runs from a checkout or the plugin
    cache. From the deployed copy (the ~/.local/bin launcher) it resolves to
    ~/.sonara, which has no src/ and no bin/ (H3), so fall back to the plugin
    Claude Code names (CLAUDE_PLUGIN_ROOT), then to the one the last install
    recorded."""
    record = install_record.read() or {}
    for cand in (paths.repo_root(), os.environ.get("CLAUDE_PLUGIN_ROOT"),
                 record.get("plugin_root")):
        if isinstance(cand, str) and is_plugin_root(cand):
            return os.path.realpath(cand)
    return None
