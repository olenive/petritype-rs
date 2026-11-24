#!/usr/bin/env python3
"""Housekeeping for a cargo or uv project: look and report, or clean stale build output.

    python3 scripts/housekeeping.py check [--offline] [limits]
    python3 scripts/housekeeping.py clean [--older-than-days N] [--yes]

`check` only looks. It prints one line per check and exits 0 when all is
well and 1 when something needs attention, so a scheduler or an agent can run
it and stay quiet otherwise. It deletes nothing and updates nothing: the
dependency checks are dry runs, and each lockfile is compared byte for byte
before and after.

`clean` lists the cargo build output that the active Rust toolchain can never
use again, because it was built before that toolchain was installed, and with
--older-than-days also the per-variant folders whose newest file is older
than that. It deletes them only when given --yes, and only inside the build
folder.

The file is the same in every repository that uses it. It needs Python 3.9 or
newer and the standard library alone, and each repository passes its own
limits from its justfile. To add a check, write a function that takes a
Project and returns a Result, and add a line for it to CHECKS.
"""

from __future__ import annotations

import argparse
import functools
import json
import os
import re
import shlex
import shutil
import stat
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Iterable

GB = 1000**3
DAY = 24 * 60 * 60

OK = "ok"
ATTENTION = "attention"
INFO = "info"
SKIPPED = "skipped"

# Every command runs with these on top of the caller's environment: no colour
# codes in the output to parse, and rustup never installs a toolchain that a
# rust-toolchain file names but this machine lacks.
QUIET_ENV = {"NO_COLOR": "1", "CARGO_TERM_COLOR": "never", "RUSTUP_AUTO_INSTALL": "0"}


# Results and how they are written


@dataclass(frozen=True)
class Result:
    """What one check found: a status, a detail with a number, and advice."""

    status: str
    detail: str
    advice: str = ""


def skipped(reason: str) -> Result:
    return Result(SKIPPED, reason)


def size(n: float) -> str:
    """Bytes as a short decimal size, as Finder shows them: 812 MB, 26.1 GB."""
    for unit, scale in (("TB", 1000**4), ("GB", GB), ("MB", 1000**2), ("KB", 1000)):
        if n >= scale:
            value = n / scale
            return f"{value:.1f} {unit}" if value < 100 else f"{value:.0f} {unit}"
    return f"{int(n)} B"


def limit(gb: float) -> str:
    return f"limit {gb:g} GB"


def day(timestamp: float) -> str:
    return time.strftime("%Y-%m-%d", time.localtime(timestamp))


def minute(timestamp: float) -> str:
    return time.strftime("%Y-%m-%d %H:%M", time.localtime(timestamp))


def first_names(names: list[str], shown: int = 5) -> str:
    """The first few names, then how many more: a, b, c and 9 more."""
    if len(names) <= shown:
        return ", ".join(names)
    return ", ".join(names[:shown]) + f" and {len(names) - shown} more"


def plural(count: int, word: str, words: str = "") -> str:
    return f"{count:,} {word if count == 1 else (words or word + 's')}"


# Running the tools


def run(args: list[str], cwd: Path | None = None, timeout: int = 900):
    """Run a program and capture what it prints. None when it is not installed."""
    try:
        return subprocess.run(
            args,
            cwd=cwd,
            env={**os.environ, **QUIET_ENV},
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            errors="replace",
            timeout=timeout,
        )
    except FileNotFoundError:
        return None
    except subprocess.TimeoutExpired:
        return subprocess.CompletedProcess(args, 124, "", f"timed out after {timeout} seconds")


def failure(proc) -> str:
    """The line that best says why a program failed."""
    if proc is None:
        return "the program is not installed"
    text = proc.stderr + "\n" + proc.stdout
    lines = [line.strip() for line in text.splitlines() if line.strip()]
    errors = [line for line in lines if line.lower().startswith("error")]
    return (errors or lines or [f"exit status {proc.returncode}"])[0]


def run_dry(args: list[str], cwd: Path, lockfile: Path):
    """Run a dry run, then make sure it left the lockfile byte for byte as it was.

    Returns the finished program, and whether the lockfile had changed and had
    to be put back.
    """
    before = lockfile.read_bytes()
    proc = run(args, cwd)
    try:
        after = lockfile.read_bytes()
    except OSError:
        after = None
    if after != before:
        lockfile.write_bytes(before)
        return proc, True
    return proc, False


DF_LINE = re.compile(r"^.*?\s+\d+\s+\d+\s+\d+\s+\d+%\s+(/.*)$")


def mount_point(path: Path) -> str | None:
    """The volume a path is on, as `df` names it: /System/Volumes/Data on a Mac."""
    proc = run(["df", "-P", str(path)], timeout=60)
    lines = proc.stdout.strip().splitlines() if proc and proc.returncode == 0 else []
    match = DF_LINE.match(lines[-1]) if len(lines) > 1 else None
    return match.group(1) if match else None


def inside(path: str | Path, folder: str | Path) -> bool:
    """Whether a path is strictly inside a folder, after resolving links."""
    path, folder = os.path.realpath(path), os.path.realpath(folder)
    return path != folder and path.startswith(folder.rstrip(os.sep) + os.sep)


def within(path: str | Path, folder: str | Path) -> bool:
    """Whether a path is a folder or inside it, after resolving links."""
    return os.path.realpath(path) == os.path.realpath(folder) or inside(path, folder)


# What the checks share: the settings, and facts found once per run


@dataclass(frozen=True)
class Settings:
    repo: Path
    offline: bool = False
    free_limit_gb: float = 50
    cargo_limit_gb: float = 50
    stale_limit_gb: float = 5
    python_limit_gb: float = 5


@dataclass(frozen=True)
class CargoFolders:
    """Where cargo puts build output: the target folder, and the build folder if separate."""

    folders: tuple[Path, ...]
    known: bool  # cargo said so; otherwise a guess
    note: str = ""  # why it is a guess


@dataclass(frozen=True)
class Toolchain:
    version: str
    installed: float  # when its compiler was written to disk, in seconds since the epoch


class Project:
    """The repository being looked at, with the facts several checks need."""

    def __init__(self, settings: Settings):
        self.settings = settings
        self.repo = settings.repo

    def has(self, name: str) -> bool:
        return (self.repo / name).is_file()

    def show(self, path: Path) -> str:
        """A path as the report shows it: relative when it is inside the repository."""
        return os.path.relpath(path, self.repo) if inside(path, self.repo) else str(path)

    @functools.cached_property
    def cargo_folders(self) -> CargoFolders:
        return locate_cargo_folders(self.repo)

    @functools.cached_property
    def toolchain(self) -> Toolchain | None:
        return find_toolchain(self.repo)

    @functools.cached_property
    def cargo_scans(self) -> list[FolderScan]:
        folders = self.cargo_folders.folders
        return [scan_build_folder(folder) for folder in folders if folder.is_dir()]


def locate_cargo_folders(repo: Path) -> CargoFolders:
    """Ask cargo where build output goes; it knows CARGO_TARGET_DIR and its config files."""
    args = ["cargo", "metadata", "--format-version", "1", "--no-deps", "--offline"]
    proc = run(args, repo, timeout=120)
    if proc is not None and proc.returncode == 0:
        try:
            meta = json.loads(proc.stdout)
            target = Path(meta["target_directory"])
            build = Path(meta.get("build_directory") or target)
            return CargoFolders(tuple(dict.fromkeys((target, build))), True)
        except (ValueError, KeyError, TypeError):
            pass
    note = "cargo is not installed" if proc is None else f"cargo metadata failed: {failure(proc)}"
    env = os.environ.get("CARGO_TARGET_DIR")
    return CargoFolders((repo / env if env else repo / "target",), False, note)


def find_toolchain(repo: Path) -> Toolchain | None:
    """The Rust toolchain active in the repository, and when it was installed.

    The install time is the modification time of the compiler in the sysroot
    that `rustc --print sysroot` names, which rustup writes when it installs.
    """
    sysroot = run(["rustc", "--print", "sysroot"], repo, timeout=120)
    version = run(["rustc", "--version"], repo, timeout=120)
    if not sysroot or sysroot.returncode or not version or version.returncode:
        return None
    root = Path(sysroot.stdout.strip())
    compiler = root / "bin" / ("rustc.exe" if os.name == "nt" else "rustc")
    try:
        installed = (compiler if compiler.exists() else root).stat().st_mtime
    except OSError:
        return None
    words = version.stdout.split()
    return Toolchain(words[1] if len(words) > 1 else version.stdout.strip(), installed)


# Scanning a cargo build folder
#
# Cargo keeps one profile folder per profile and target: target/debug,
# target/release, target/<triple>/debug, and others under target/tests for
# trybuild. Each holds a .cargo-lock file. Inside a profile folder the build
# output is split into units, which cargo builds, and a clean keeps or
# deletes, whole: since cargo 1.100.0 a per-variant folder
# build/<crate>/<hash>/ holds a variant's fingerprint and output together.
# The layout before kept build/<crate>-<hash>/, .fingerprint/<crate>-<hash>/,
# incremental/<crate>-<hash>/ and loose files in deps/. Anything in the build
# folder outside the profile folders (docs, packaging, logs someone left
# there) is measured and never deleted.

CARGO_LOCKS = {".cargo-lock", ".cargo-build-lock", ".cargo-artifact-lock"}
PROFILE_PARTS = {"build", "deps", ".fingerprint", "incremental"}
UNIT_PARENTS = {"deps", ".fingerprint", "incremental", "examples"}
OLD_LAYOUT_UNIT = re.compile(r"-[0-9a-f]{16}$")
PROFILE_DEPTH = 5  # target/tests/trybuild/<triple>/debug is four below the build folder


@dataclass(frozen=True)
class Unit:
    """Build output kept or deleted whole: a folder, or a loose file."""

    path: str
    profile: str  # the profile folder it is in, relative to the build folder
    newest: float  # newest modification time of any file in it
    bytes: int  # on disk, a file with several hard links counted once
    files: int
    folder: bool
    variant: bool  # a per-variant folder, build/<crate>/<hash>/


@dataclass(frozen=True)
class FolderScan:
    folder: Path  # with links resolved
    bytes: int
    units: list[Unit]


class DiskUsage:
    """Adds up bytes on disk, counting a file with several hard links once, as `du` does."""

    def __init__(self):
        self._seen: set[tuple[int, int]] = set()

    def of(self, st: os.stat_result) -> int:
        if st.st_nlink > 1:
            key = (st.st_dev, st.st_ino)
            if key in self._seen:
                return 0
            self._seen.add(key)
        blocks = getattr(st, "st_blocks", None)
        return blocks * 512 if blocks is not None else st.st_size


def list_folder(path: str) -> list[os.DirEntry]:
    try:
        with os.scandir(path) as found:
            return list(found)
    except OSError:
        return []


def measure(path: str, usage: DiskUsage | None = None) -> tuple[float, int, int]:
    """Newest modification time, bytes on disk and number of files in a folder.

    Symbolic links are neither followed nor counted.
    """
    usage = usage or DiskUsage()
    newest, total, files = 0.0, 0, 0
    pending = [path]
    while pending:
        for entry in list_folder(pending.pop()):
            try:
                if entry.is_symlink():
                    continue
                if entry.is_dir(follow_symlinks=False):
                    pending.append(entry.path)
                    continue
                st = entry.stat(follow_symlinks=False)
            except OSError:
                continue
            newest = max(newest, st.st_mtime)
            total += usage.of(st)
            files += 1
    return newest, total, files


def folder_unit(path: str, profile: str, usage: DiskUsage, variant: bool = False) -> Unit:
    newest, total, files = measure(path, usage)
    return Unit(path, profile, newest, total, files, folder=True, variant=variant)


def file_unit(entry: os.DirEntry, profile: str, usage: DiskUsage) -> Unit:
    try:
        st = entry.stat(follow_symlinks=False)
    except OSError:  # gone meanwhile: a unit of no files, which nothing selects
        return Unit(entry.path, profile, 0.0, 0, 0, folder=False, variant=False)
    return Unit(entry.path, profile, st.st_mtime, usage.of(st), 1, folder=False, variant=False)


def units_in(path: str, profile: str, usage: DiskUsage, variant: bool = False) -> list[Unit]:
    """Each entry of a folder as a unit of its own."""
    units = []
    for entry in list_folder(path):
        if entry.is_symlink():
            continue
        if entry.is_dir(follow_symlinks=False):
            units.append(folder_unit(entry.path, profile, usage, variant))
        else:
            units.append(file_unit(entry, profile, usage))
    return units


def build_units(path: str, profile: str, usage: DiskUsage) -> list[Unit]:
    """The units in a profile's build folder, in either layout."""
    units = []
    for entry in list_folder(path):
        if entry.is_symlink():
            continue
        if not entry.is_dir(follow_symlinks=False):
            units.append(file_unit(entry, profile, usage))
        elif OLD_LAYOUT_UNIT.search(entry.name):
            units.append(folder_unit(entry.path, profile, usage))
        else:
            units.extend(units_in(entry.path, profile, usage, variant=True))
    return units


def is_profile(found: list[os.DirEntry]) -> bool:
    names = {entry.name for entry in found}
    return bool(names & CARGO_LOCKS) and bool(names & PROFILE_PARTS)


def profile_units(path: str, profile: str, found: list[os.DirEntry], usage: DiskUsage) -> list[Unit]:
    units = []
    for entry in found:
        if entry.is_symlink() or entry.name in CARGO_LOCKS:
            continue
        if not entry.is_dir(follow_symlinks=False):
            units.append(file_unit(entry, profile, usage))
        elif entry.name == "build":
            units.extend(build_units(entry.path, profile, usage))
        elif entry.name in UNIT_PARENTS:
            units.extend(units_in(entry.path, profile, usage))
        else:
            units.append(folder_unit(entry.path, profile, usage))
    return units


def scan_build_folder(folder: Path) -> FolderScan:
    """Measure a cargo build folder and split its profile folders into units."""
    root = os.path.realpath(folder)
    usage = DiskUsage()
    total, units = 0, []
    pending = [(root, 0)]
    while pending:
        path, depth = pending.pop()
        found = list_folder(path)
        if depth <= PROFILE_DEPTH and is_profile(found):
            here = profile_units(path, os.path.relpath(path, root), found, usage)
            units.extend(here)
            total += sum(unit.bytes for unit in here)
            continue
        for entry in found:
            try:
                if entry.is_symlink():
                    continue
                if entry.is_dir(follow_symlinks=False):
                    pending.append((entry.path, depth + 1))
                else:
                    total += usage.of(entry.stat(follow_symlinks=False))
            except OSError:
                continue
    return FolderScan(Path(root), total, sorted(units, key=lambda unit: unit.path))


@dataclass(frozen=True)
class Candidate:
    """A unit a clean would delete, and why."""

    unit: Unit
    cutoff: float  # everything in it is older than this
    reason: str  # "toolchain" or "unused"


def select_stale(
    units: Iterable[Unit], toolchain_installed: float, unused_since: float | None = None
) -> list[Candidate]:
    """The units built before the toolchain was installed, and, when given a
    time, the per-variant folders whose newest file is older than it."""
    chosen = []
    for unit in units:
        if unit.files == 0:
            continue
        if unit.newest < toolchain_installed:
            chosen.append(Candidate(unit, toolchain_installed, "toolchain"))
        elif unused_since is not None and unit.variant and unit.newest < unused_since:
            chosen.append(Candidate(unit, unused_since, "unused"))
    return chosen


# Reading what the package tools print

CARGO_LINE = re.compile(
    r"^\s*(Updating|Downgrading|Adding|Removing|Unchanged)\s+(\S+)\s+v(\S+)"
    r"(?:\s+->\s+v(\S+))?(?:\s+\((.*)\))?\s*$"
)
CARGO_UNNAMED = re.compile(r"to see (\d+) unchanged dependenc(?:y|ies) behind latest")
UV_LINE = re.compile(r"^(Update|Add|Remove)\s+(\S+)\s+v(\S+)(?:\s+->\s+v(\S+))?\s*$")
RUSTUP_LINE = re.compile(r"^(\S+) - (update available|up to date)\s*:\s*(.*)$", re.IGNORECASE)


@dataclass(frozen=True)
class CargoUpdates:
    newer: list[str]  # crates with a newer version the requirements allow
    behind: list[str]  # crates with a newer version the requirements or Rust version exclude
    behind_unnamed: int = 0  # counted by cargo without names


def parse_cargo_update(text: str) -> CargoUpdates:
    """Read `cargo update --dry-run --verbose`, as cargo 1.100.0 prints it.

    `Updating name vOLD -> vNEW` is an update within the requirements. A note
    such as `(available: v2.0.0)` or `(available: v1.4.0, requires Rust 1.98)`,
    on that line or on an `Unchanged` line, is a newer version that they
    exclude. Without --verbose cargo prints only how many of those there are.
    """
    newer, behind, unnamed = [], [], 0
    for line in text.splitlines():
        match = CARGO_LINE.match(line)
        if match:
            verb, name, _old, _new, note = match.groups()
            if verb == "Updating":
                newer.append(name)
            if note and "available" in note:
                behind.append(name)
            continue
        hidden = CARGO_UNNAMED.search(line)
        if hidden:
            unnamed += int(hidden.group(1))
    return CargoUpdates(newer, behind, unnamed)


@dataclass(frozen=True)
class UvChanges:
    updated: list[str]
    added: list[str]
    removed: list[str]


def parse_uv_lock(text: str) -> UvChanges:
    """Read `uv lock --upgrade --dry-run`, whose lines are
    `Update name vOLD -> vNEW`, `Add name vX` and `Remove name vX`."""
    changes: dict[str, list[str]] = {"Update": [], "Add": [], "Remove": []}
    for line in text.splitlines():
        match = UV_LINE.match(line.strip())
        if match:
            changes[match.group(1)].append(match.group(2))
    return UvChanges(changes["Update"], changes["Add"], changes["Remove"])


def parse_tmutil_snapshots(text: str) -> list[str]:
    """Read `tmutil listlocalsnapshots`: a heading ending in a colon, then one snapshot a line."""
    lines = [line.strip() for line in text.splitlines()]
    return [line for line in lines if line and not line.endswith(":")]


def parse_rustup_check(text: str) -> list[tuple[str, bool, str]]:
    """Read `rustup check`: a toolchain or `rustup`, whether it has an update, and the versions."""
    found = []
    for line in text.splitlines():
        match = RUSTUP_LINE.match(line.strip())
        if match:
            versions = re.sub(r"\s*\([^)]*\)", "", match.group(3)).strip()
            found.append((match.group(1), match.group(2).lower() == "update available", versions))
    return found


# The checks


def free_space(project: Project) -> Result:
    gb = project.settings.free_limit_gb
    free = shutil.disk_usage(project.repo).free
    volume = mount_point(project.repo) or str(project.repo)
    detail = f"{size(free)} free on {volume}, {limit(gb)}"
    if free < gb * GB:
        advice = "the lines below say what can go; Time Machine snapshots may hold deleted space"
        return Result(ATTENTION, detail, advice)
    return Result(OK, detail)


def cargo_build_folder(project: Project) -> Result:
    if not project.has("Cargo.toml"):
        return skipped("no Cargo.toml")
    located = project.cargo_folders
    scans = project.cargo_scans
    guess = "" if located.known else f" ({located.note}; assumed)"
    if not scans:
        return Result(OK, f"no build folder yet at {project.show(located.folders[0])}{guess}")
    gb = project.settings.cargo_limit_gb
    total = sum(scan.bytes for scan in scans)
    where = " and ".join(project.show(scan.folder) for scan in scans)
    detail = f"{size(total)} in {where}, {limit(gb)}{guess}"
    if total > gb * GB:
        advice = "`just housekeeping-clean` lists the part that is stale; `cargo clean` empties it all"
        return Result(ATTENTION, detail, advice)
    return Result(OK, detail)


def stale_build_output(project: Project) -> Result:
    if not project.has("Cargo.toml"):
        return skipped("no Cargo.toml")
    toolchain = project.toolchain
    if toolchain is None:
        return skipped("rustc is not installed or did not answer")
    if not project.cargo_scans:
        return Result(OK, "no build folder yet")
    amount = sum(
        found.unit.bytes
        for scan in project.cargo_scans
        for found in select_stale(scan.units, toolchain.installed)
    )
    gb = project.settings.stale_limit_gb
    installed = day(toolchain.installed)
    detail = f"{size(amount)} built before Rust {toolchain.version} was installed on {installed}, {limit(gb)}"
    if amount > gb * GB:
        return Result(ATTENTION, detail, "`just housekeeping-clean` lists it, and deletes it when given --yes")
    return Result(OK, detail)


# Python build leftovers: caches found anywhere in the checkout, and the
# environment, dist/ and build/ at its top. Folders whose names start with a
# dot are not searched, apart from the caches named here.
PYTHON_CACHES = {
    "__pycache__",
    ".pytest_cache",
    ".ruff_cache",
    ".mypy_cache",
    ".hypothesis",
    "__marimo__",
    ".ipynb_checkpoints",
}
PYTHON_TOP_LEVEL = ("dist", "build")
NOT_SEARCHED = {"node_modules", "target"}


def python_caches(repo: Path, environment: Path) -> list[str]:
    found, pending = [], [str(repo)]
    while pending:
        here = pending.pop()
        for entry in list_folder(here):
            if entry.is_symlink() or not entry.is_dir(follow_symlinks=False):
                continue
            if entry.name in PYTHON_CACHES or entry.name.endswith(".egg-info"):
                found.append(entry.path)
            elif entry.name.startswith(".") or entry.name in NOT_SEARCHED:
                continue
            elif entry.path == str(environment):
                continue
            elif here == str(repo) and entry.name in PYTHON_TOP_LEVEL:
                continue
            else:
                pending.append(entry.path)
    return found


def python_build_folders(project: Project) -> Result:
    if not project.has("pyproject.toml"):
        return skipped("no pyproject.toml")
    repo = project.repo
    environment = repo / os.environ.get("UV_PROJECT_ENVIRONMENT", ".venv")
    parts = []
    if environment.is_dir():
        parts.append((project.show(environment), measure(str(environment))[1]))
    for name in PYTHON_TOP_LEVEL:
        if (repo / name).is_dir():
            parts.append((name, measure(str(repo / name))[1]))
    caches = python_caches(repo, environment)
    if caches:
        parts.append((plural(len(caches), "cache folder"), sum(measure(path)[1] for path in caches)))
    total = sum(amount for _, amount in parts)
    gb = project.settings.python_limit_gb
    shown = ", ".join(f"{name} {size(amount)}" for name, amount in parts) or "nothing built yet"
    detail = f"{size(total)} ({shown}), {limit(gb)}"
    if total > gb * GB:
        advice = "delete the environment and `uv sync` makes it again; dist and the caches can go"
        return Result(ATTENTION, detail, advice)
    return Result(OK, detail)


def time_machine_snapshots(project: Project) -> Result:
    if sys.platform != "darwin":
        return skipped("not macOS")
    if shutil.which("tmutil") is None:
        return skipped("tmutil is not installed")
    volume = mount_point(project.repo) or "/"
    proc = run(["tmutil", "listlocalsnapshots", volume], timeout=120)
    if proc is None or proc.returncode != 0:
        return skipped(f"tmutil listlocalsnapshots failed: {failure(proc)}")
    snapshots = parse_tmutil_snapshots(proc.stdout)
    detail = f"{plural(len(snapshots), 'local snapshot')} on {volume}"
    if not snapshots:
        return Result(INFO, detail)
    advice = (
        "they can hold deleted space for up to a day; deleting them is your decision, "
        f"with `tmutil deletelocalsnapshots {shlex.quote(volume)}`"
    )
    return Result(INFO, detail, advice)


def rust_dependencies(project: Project) -> Result:
    if not project.has("Cargo.toml"):
        return skipped("no Cargo.toml")
    if not project.has("Cargo.lock"):
        return skipped("no Cargo.lock")
    if shutil.which("cargo") is None:
        return skipped("cargo is not installed")
    args = ["cargo", "update", "--dry-run", "--verbose"]
    proc, restored = run_dry(args, project.repo, project.repo / "Cargo.lock")
    if restored:
        detail = "cargo update --dry-run changed Cargo.lock, and the old one was put back"
        return Result(ATTENTION, detail, "run it by hand to see why")
    if proc is None or proc.returncode != 0:
        detail = f"cargo update --dry-run failed: {failure(proc)}"
        return Result(ATTENTION, detail, "run it by hand to see why, or pass --offline")
    found = parse_cargo_update(proc.stderr + "\n" + proc.stdout)
    newer = f"{plural(len(found.newer), 'update')} within the version requirements"
    if found.newer:
        newer += f" ({first_names(found.newer)})"
    behind = f"{len(found.behind) + found.behind_unnamed} with a newer version outside them"
    if found.behind:
        behind += f" ({first_names(found.behind)})"
    return Result(INFO, f"{newer}; {behind}")


def rust_toolchain(project: Project) -> Result:
    if not project.has("Cargo.toml"):
        return skipped("no Cargo.toml")
    if shutil.which("rustup") is None:
        return skipped("rustup is not installed")
    proc = run(["rustup", "check"], project.repo, timeout=300)
    found = parse_rustup_check(proc.stdout + "\n" + proc.stderr) if proc else []
    if not found:
        detail = f"rustup check failed: {failure(proc)}"
        return Result(ATTENTION, detail, "run it by hand to see why, or pass --offline")
    active = run(["rustup", "show", "active-toolchain"], project.repo, timeout=120)
    words = active.stdout.split() if active and active.returncode == 0 else []
    active_name = words[0] if words else ""
    version = project.toolchain.version if project.toolchain else active_name or "unknown"
    channels = [(name, update, versions) for name, update, versions in found if name != "rustup"]
    pinned = active_name[:1].isdigit()
    parts = [f"Rust {version} is pinned" if pinned else f"Rust {version} ({active_name})"]
    updates = [f"{name} {versions}" for name, update, versions in channels if update]
    if updates:
        parts.append("newer available: " + ", ".join(updates))
    elif channels:
        parts.append(", ".join(name for name, _, _ in channels) + " up to date")
    elif pinned:
        parts.append("rustup check compares channels such as stable only, and none is installed")
    for name, update, versions in found:
        if name == "rustup":
            parts.append(f"rustup {versions} available" if update else f"rustup {versions} up to date")
    return Result(INFO, "; ".join(parts))


def ci_audit(repo: Path) -> str:
    """The CI workflow that runs an advisory audit, if there is one."""
    for workflow in sorted((repo / ".github" / "workflows").glob("*.y*ml")):
        try:
            if "audit" in workflow.read_text(errors="replace").lower():
                return str(workflow.relative_to(repo))
        except OSError:
            continue
    return ""


def rust_advisories(project: Project) -> Result:
    if not project.has("Cargo.toml"):
        return skipped("no Cargo.toml")
    if not project.has("Cargo.lock"):
        return skipped("no Cargo.lock")
    if shutil.which("cargo-audit") is None:
        workflow = ci_audit(project.repo)
        also = f"; CI runs an audit, {workflow}" if workflow else ""
        return skipped(f"cargo audit is not installed{also}")
    proc, restored = run_dry(["cargo", "audit", "--json"], project.repo, project.repo / "Cargo.lock")
    if restored:
        detail = "cargo audit changed Cargo.lock, and the old one was put back"
        return Result(ATTENTION, detail, "run it by hand to see why")
    try:
        report = json.loads(proc.stdout) if proc else None
        found = report["vulnerabilities"]["list"]
        names = [f"{item['package']['name']} ({item['advisory']['id']})" for item in found]
        warnings = sum(len(items) for items in (report.get("warnings") or {}).values())
        crates = (report.get("lockfile") or {}).get("dependency-count", "the")
    except (ValueError, KeyError, TypeError, AttributeError):
        return Result(ATTENTION, f"cargo audit failed: {failure(proc)}", "run `cargo audit` by hand to see why")
    if names:
        detail = f"{plural(len(names), 'advisory', 'advisories')}: {first_names(names)}"
        return Result(ATTENTION, detail, "`cargo audit` says what each one is and which version fixes it")
    tail = f", and {plural(warnings, 'warning')} such as unmaintained or yanked crates" if warnings else ""
    return Result(OK, f"no known advisories against {crates} crates in Cargo.lock{tail}")


def python_dependencies(project: Project) -> Result:
    if not project.has("uv.lock"):
        return skipped("no uv.lock")
    if shutil.which("uv") is None:
        return skipped("uv is not installed")
    # --no-build: no package's build code runs to learn its requirements.
    # --no-python-downloads: no Python is installed to resolve with.
    args = ["uv", "lock", "--upgrade", "--dry-run", "--no-build", "--no-python-downloads"]
    proc, restored = run_dry(args, project.repo, project.repo / "uv.lock")
    if restored:
        detail = "uv lock --dry-run changed uv.lock, and the old one was put back"
        return Result(ATTENTION, detail, "run it by hand to see why")
    if proc is None or proc.returncode != 0:
        detail = f"uv lock --upgrade --dry-run failed: {failure(proc)}"
        return Result(ATTENTION, detail, "run it by hand to see why, or pass --offline")
    found = parse_uv_lock(proc.stderr + "\n" + proc.stdout)
    detail = f"{plural(len(found.updated), 'newer version')} within the requirements"
    if found.updated:
        detail += f" ({first_names(found.updated)})"
    if found.added or found.removed:
        detail += f"; with them {len(found.added)} added and {len(found.removed)} removed"
    return Result(INFO, detail)


@dataclass(frozen=True)
class Check:
    name: str
    run: Callable[[Project], Result]
    network: bool = False


CHECKS = (
    Check("Free space", free_space),
    Check("Cargo build folder", cargo_build_folder),
    Check("Stale build output", stale_build_output),
    Check("Python build folders", python_build_folders),
    Check("Time Machine", time_machine_snapshots),
    Check("Rust dependencies", rust_dependencies, network=True),
    Check("Rust toolchain", rust_toolchain, network=True),
    Check("Rust advisories", rust_advisories, network=True),
    Check("Python dependencies", python_dependencies, network=True),
)


def check_command(settings: Settings) -> int:
    project = Project(settings)
    width = max(len(check.name) for check in CHECKS)
    status_width = len(ATTENTION)
    indent = " " * (2 + width + 2 + status_width + 2)
    print(f"Housekeeping in {settings.repo}, {minute(time.time())}")
    print()
    attention = 0
    for check in CHECKS:
        if check.network and settings.offline:
            result = skipped("--offline, and it needs the network")
        else:
            try:
                result = check.run(project)
            except Exception as error:  # one broken check must not hide the others
                detail = f"the check itself failed: {error!r}"
                result = Result(ATTENTION, detail, "this is a fault in scripts/housekeeping.py")
        attention += result.status == ATTENTION
        print(f"  {check.name:<{width}}  {result.status:<{status_width}}  {result.detail}", flush=True)
        if result.advice:
            print(f"{indent}{result.advice}", flush=True)
    print()
    print(f"{plural(attention, 'check needs', 'checks need')} attention." if attention else "All is well.")
    return 1 if attention else 0


# Cleaning

RUST_TOOLS = {"cargo", "rustc", "rustdoc", "clippy-driver", "sccache"}


def process_cwd(pid: str) -> str | None:
    """The working folder of a process, or None when it cannot be told."""
    if os.path.isdir("/proc"):
        try:
            return os.readlink(f"/proc/{pid}/cwd")
        except OSError:
            return None
    proc = run(["lsof", "-a", "-p", pid, "-d", "cwd", "-Fn"], timeout=60)
    for line in proc.stdout.splitlines() if proc else []:
        if line.startswith("n"):
            return line[1:]
    return None


def names_a_folder(args: str, folders: list[Path]) -> bool:
    """Whether a command line names a path in one of the folders, as rustc's --out-dir does."""
    paths = [token for token in re.split(r"[\s=,]+", args) if token.startswith("/")]
    return any(within(path, folder) for path in paths for folder in folders)


def rust_processes_using(folders: Iterable[Path], repo: Path) -> list[str]:
    """The cargo and rustc processes that write to a build folder or work in the repository.

    A rustc counts when its command line names a path in a build folder. A
    cargo counts when it does, or when it works in the repository or a build
    folder, or when its working folder cannot be read.
    """
    folders = list(folders)
    proc = run(["ps", "-axo", "pid=,ucomm="], timeout=60)
    if proc is None or proc.returncode != 0:
        return ["(ps failed, so running builds cannot be told apart)"]
    busy = []
    for line in proc.stdout.splitlines():
        pid, _, name = line.strip().partition(" ")
        name = name.strip()
        is_cargo = name == "cargo" or name.startswith("cargo-")
        if not (is_cargo or name in RUST_TOOLS) or pid == str(os.getpid()):
            continue
        shown = run(["ps", "-ww", "-o", "args=", "-p", pid], timeout=60)
        args = shown.stdout.strip() if shown and shown.returncode == 0 else ""
        if not args:
            continue  # it ended meanwhile
        if names_a_folder(args, folders):
            busy.append(f"{pid} {args[:120]}")
        elif is_cargo:
            cwd = process_cwd(pid)
            if cwd is None:
                busy.append(f"{pid} {args[:120]} (its working folder cannot be read)")
            elif within(cwd, repo) or any(within(cwd, folder) for folder in folders):
                busy.append(f"{pid} {args[:120]}")
    return busy


def named_by_environment(folder: Path) -> bool:
    """Whether CARGO_TARGET_DIR, or CARGO_BUILD_BUILD_DIR, names this folder."""
    named = [os.environ.get(name) for name in ("CARGO_TARGET_DIR", "CARGO_BUILD_BUILD_DIR")]
    return any(value and os.path.realpath(value) == os.path.realpath(folder) for value in named)


def remove_empty(path: str) -> bool:
    try:
        os.rmdir(path)
        return True
    except OSError:
        return False


def delete(candidate: Candidate, root: str) -> tuple[int, int, int] | None:
    """Delete one unit, if it is still entirely older than its cutoff.

    Returns how many files went, how many could not be deleted, and how many
    folders went, or None when the unit was kept because it changed. Only
    regular files are deleted; symbolic links are left where they are.
    """
    unit, cutoff = candidate.unit, candidate.cutoff
    if os.path.realpath(unit.path) != unit.path or not inside(unit.path, root):
        return None
    if not unit.folder:
        try:
            st = os.lstat(unit.path)
            if not stat.S_ISREG(st.st_mode) or st.st_mtime >= cutoff:
                return None
            os.unlink(unit.path)
            return 1, 0, 0
        except OSError:
            return 0, 1, 0
    if measure(unit.path)[0] >= cutoff:
        return None
    files = failed = folders = 0
    for here, dirs, names in os.walk(unit.path, topdown=False, followlinks=False):
        for name in names:
            path = os.path.join(here, name)
            try:
                st = os.lstat(path)
                if stat.S_ISREG(st.st_mode) and st.st_mtime < cutoff:
                    os.unlink(path)
                    files += 1
            except OSError:
                failed += 1
        for name in dirs:
            path = os.path.join(here, name)
            if not os.path.islink(path):
                folders += remove_empty(path)
    folders += remove_empty(unit.path)
    return files, failed, folders


@dataclass(frozen=True)
class CleanSettings:
    repo: Path
    yes: bool = False
    older_than_days: int | None = None
    outside_repo: bool = False
    ignore_running: bool = False


def refusal(settings: CleanSettings, located: CargoFolders, toolchain: Toolchain | None) -> str:
    """Why a clean must not go ahead, or nothing when it may."""
    if not located.known:
        return f"cannot tell where the build folder is, {located.note}"
    if toolchain is None:
        return "cannot tell which Rust toolchain is active, because rustc did not answer"
    for folder in located.folders:
        if inside(folder, settings.repo):
            continue
        shown = f"{folder}, a link to {os.path.realpath(folder)}," if os.path.islink(folder) else str(folder)
        if not named_by_environment(folder):
            return f"the build folder {shown} is outside the repository, and CARGO_TARGET_DIR does not name it"
        if not settings.outside_repo:
            return (
                f"the build folder {shown} is outside the repository; "
                "CARGO_TARGET_DIR names it, so pass --outside-repo to confirm"
            )
    if settings.ignore_running:
        return ""
    busy = rust_processes_using(located.folders, settings.repo)
    if busy:
        running = "".join(f"\n  {line}" for line in busy)
        return (
            f"cargo or rustc is running with this build folder:{running}\n"
            "Wait for it to finish, or pass --ignore-running to clean anyway."
        )
    return ""


def summary(chosen: list[Candidate]) -> str:
    """How much a list of units is: folders, loose files, files in all, and bytes."""
    if not chosen:
        return "nothing"
    folders = sum(found.unit.folder for found in chosen)
    files = sum(found.unit.files for found in chosen)
    amount = size(sum(found.unit.bytes for found in chosen))
    loose = plural(len(chosen) - folders, "loose file")
    return f"{plural(folders, 'folder')} and {loose}, {plural(files, 'file')} in all, about {amount}"


def print_listing(
    scans: list[FolderScan],
    chosen: list[tuple[FolderScan, Candidate]],
    toolchain: Toolchain,
    days: int | None,
    unused_since: float | None,
) -> None:
    """Every unit a clean would delete, one line each, then how much that is."""
    folders = " and ".join(str(scan.folder) for scan in scans) or "no build folder yet"
    print(f"Stale build output in {folders}")
    installed = minute(toolchain.installed)
    print(f"Rust {toolchain.version} was installed on {installed}; output built before then cannot be used again.")
    if days:
        before = minute(unused_since)
        print(f"Also per-variant folders with no file newer than {plural(days, 'day')}, so before {before}.")
    print()
    for scan, found in chosen:
        unit = found.unit
        why = "older Rust" if found.reason == "toolchain" else "unused"
        name = os.path.relpath(unit.path, scan.folder) + ("/" if unit.folder else "")
        print(f"  {size(unit.bytes):>8}  {day(unit.newest)}  {why:<10}  {name}")
    if chosen:
        print()
    candidates = [found for _, found in chosen]
    print(f"Built before Rust {toolchain.version}: {summary([c for c in candidates if c.reason == 'toolchain'])}")
    if days:
        print(f"Not built for {plural(days, 'day')}: {summary([c for c in candidates if c.reason == 'unused'])}")
    profiles: dict[str, int] = {}
    for found in candidates:
        profiles[found.unit.profile] = profiles.get(found.unit.profile, 0) + found.unit.bytes
    if profiles:
        print("By profile folder: " + ", ".join(f"{name} {size(n)}" for name, n in sorted(profiles.items())))


def clean_command(settings: CleanSettings) -> int:
    repo = settings.repo
    if not (repo / "Cargo.toml").is_file():
        print(f"Nothing to clean in {repo}: there is no Cargo.toml, and a clean covers cargo build output only.")
        return 0
    located = locate_cargo_folders(repo)
    toolchain = find_toolchain(repo)
    reason = refusal(settings, located, toolchain)
    if reason:
        print(f"Refusing to clean: {reason}", file=sys.stderr)
        return 1

    days = settings.older_than_days
    unused_since = time.time() - days * DAY if days else None
    scans = [scan_build_folder(folder) for folder in located.folders if folder.is_dir()]
    chosen = [
        (scan, found)
        for scan in scans
        for found in select_stale(scan.units, toolchain.installed, unused_since)
    ]
    print_listing(scans, chosen, toolchain, days, unused_since)
    if not settings.yes:
        everything = summary([found for _, found in chosen])
        print(f"In all: {everything}. Nothing was deleted; run again with --yes to delete it.")
        return 0

    files = failed = folders = kept = 0
    for scan, found in chosen:
        outcome = delete(found, str(scan.folder))
        if outcome is None:
            kept += 1
            continue
        files, failed, folders = files + outcome[0], failed + outcome[1], folders + outcome[2]
        if found.unit.variant:  # build/<crate>/ goes with its last variant
            folders += remove_empty(os.path.dirname(found.unit.path))
    print(f"Deleted {plural(files, 'file')} and {plural(folders, 'empty folder')}.")
    if kept:
        listed = plural(kept, "listed folder or file", "listed folders or files")
        print(f"Kept {listed}, which a build changed meanwhile.")
    if failed:
        print(f"Could not delete {plural(failed, 'file')}.")
        return 1
    return 0


# The command line


def positive_int(text: str) -> int:
    value = int(text)
    if value < 1:
        raise argparse.ArgumentTypeError("must be 1 or more")
    return value


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    commands = parser.add_subparsers(dest="command", required=True)
    here = "(default: the current folder)"

    check = commands.add_parser(
        "check", help="look and report; delete nothing; exit 1 when something needs attention"
    )
    check.add_argument("--repo", type=Path, default=Path("."), help=f"the repository to look at {here}")
    check.add_argument("--offline", action="store_true", help="skip the checks that need the network")
    check.add_argument(
        "--free-limit-gb", type=float, default=Settings.free_limit_gb,
        help="free space below this needs attention (default: %(default)s)",
    )
    check.add_argument(
        "--cargo-limit-gb", type=float, default=Settings.cargo_limit_gb,
        help="a cargo build folder over this needs attention (default: %(default)s)",
    )
    check.add_argument(
        "--stale-limit-gb", type=float, default=Settings.stale_limit_gb,
        help="stale cargo build output over this needs attention (default: %(default)s)",
    )
    check.add_argument(
        "--python-limit-gb", type=float, default=Settings.python_limit_gb,
        help="Python build folders over this need attention (default: %(default)s)",
    )

    clean = commands.add_parser("clean", help="list stale cargo build output; delete it only with --yes")
    clean.add_argument("--repo", type=Path, default=Path("."), help=f"the repository to clean {here}")
    clean.add_argument("--yes", action="store_true", help="delete what is listed; without it nothing is deleted")
    clean.add_argument(
        "--older-than-days", type=positive_int, metavar="N",
        help="also delete per-variant folders whose newest file is older than N days",
    )
    clean.add_argument(
        "--outside-repo", action="store_true",
        help="allow a build folder outside the repository, when CARGO_TARGET_DIR names it",
    )
    clean.add_argument(
        "--ignore-running", action="store_true",
        help="clean even while cargo or rustc runs with the build folder",
    )
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    repo = args.repo.resolve()
    if args.command == "check":
        return check_command(
            Settings(
                repo=repo,
                offline=args.offline,
                free_limit_gb=args.free_limit_gb,
                cargo_limit_gb=args.cargo_limit_gb,
                stale_limit_gb=args.stale_limit_gb,
                python_limit_gb=args.python_limit_gb,
            )
        )
    return clean_command(
        CleanSettings(
            repo=repo,
            yes=args.yes,
            older_than_days=args.older_than_days,
            outside_repo=args.outside_repo,
            ignore_running=args.ignore_running,
        )
    )


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
