#!/usr/bin/env python3
"""Compile a temporary local Nausicaa consumer without editing either repo."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--nausicaa", type=Path, required=True)
args = parser.parse_args()
root = Path(__file__).resolve().parents[1]
core = (args.nausicaa.resolve() / "crates/harness-core").resolve(strict=True)
environment = dict(os.environ, CARGO_TARGET_DIR=str(root / "target/local-host"))

# JSON strings are valid TOML basic strings for these filesystem paths. They
# are written as data, never interpolated into a shell command.
def quoted(path):
    return json.dumps(str(path), ensure_ascii=False)

with tempfile.TemporaryDirectory(prefix="ohmu-local-host-") as directory:
    consumer = Path(directory)
    (consumer / "src").mkdir()
    (consumer / "Cargo.toml").write_text(f'''[package]
name = "ohmu-local-host-check"
version = "0.0.0"
edition = "2024"

[workspace]

[dependencies]
agent-harness-core = {{ path = {quoted(core)} }}
ohmu-engine = {{ path = {quoted(root / "crates/ohmu-engine")} }}
ohmu-nausicaa = {{ path = {quoted(root / "crates/ohmu-nausicaa")} }}

[patch."https://github.com/Ceskoslov/Nausicaa.git"]
agent-harness-core = {{ path = {quoted(core)} }}
''', encoding="utf-8")
    (consumer / "src/main.rs").write_text('''use std::sync::Arc;
use agent_harness_core::{RegistryError, ToolRegistry};
use ohmu_engine::Wiki;
fn main() {
    let _register: fn(&mut ToolRegistry, Arc<Wiki>) -> Result<(), RegistryError> =
        ohmu_nausicaa::register_read_tools;
}
''', encoding="utf-8")
    def cargo(*arguments, capture=False):
        result = subprocess.run(["cargo", *arguments, "--manifest-path", str(consumer / "Cargo.toml")],
                                check=False, env=environment, cwd=root,
                                text=True, capture_output=capture)
        if result.returncode:
            if capture:
                print(result.stderr, file=sys.stderr, end="")
            raise SystemExit(result.returncode)
        return result

    cargo("generate-lockfile", "--offline")
    cargo("check", "--locked", "--offline")
    metadata = json.loads(cargo("metadata", "--format-version", "1", "--locked", "--offline", capture=True).stdout)
    packages = [p for p in metadata["packages"] if p["name"] == "agent-harness-core"]
    if len(packages) != 1 or packages[0]["source"] is not None or Path(packages[0]["manifest_path"]).resolve() != core / "Cargo.toml":
        raise SystemExit("Expected exactly one local agent-harness-core; dependency sources diverged.")
    print(f"Local host compatibility passed: one harness core at {core}.")
