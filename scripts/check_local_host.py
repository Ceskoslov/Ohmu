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
parser.add_argument("--with-provider", action="store_true", help="Also unify native provider and workflow composition")
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
    provider_dependency = f'agent-harness-provider-openai = {{ path = {quoted(core.parent / "provider-openai")}, features = ["reqwest-transport"] }}\n' if args.with_provider else ''
    provider_patch = f'agent-harness-provider-openai = {{ path = {quoted(core.parent / "provider-openai")} }}\n' if args.with_provider else ''
    (consumer / "Cargo.toml").write_text(f'''[package]
name = "ohmu-local-host-check"
version = "0.0.0"
edition = "2024"

[workspace]

[dependencies]
agent-harness-core = {{ path = {quoted(core)} }}
ohmu-engine = {{ path = {quoted(root / "crates/ohmu-engine")} }}
ohmu-nausicaa = {{ path = {quoted(root / "crates/ohmu-nausicaa")} }}

{provider_dependency}
[patch."https://github.com/Ceskoslov/Nausicaa.git"]
agent-harness-core = {{ path = {quoted(core)} }}
{provider_patch}
''', encoding="utf-8")
    (consumer / "src/main.rs").write_text('''use std::sync::Arc;
use agent_harness_core::{RegistryError, ToolRegistry};
use ohmu_engine::Wiki;
fn main() {
    let _register: fn(&mut ToolRegistry, Arc<Wiki>) -> Result<(), RegistryError> =
        ohmu_nausicaa::register_read_tools;
}
''', encoding="utf-8")
    if args.with_provider:
        with (consumer / "src/main.rs").open("a") as source:
            source.write('''
#[allow(dead_code)]
fn provider_boundary(config: agent_harness_provider_openai::OpenAiConfig) -> std::sync::Arc<dyn agent_harness_core::ModelAdapter> {
    std::sync::Arc::new(agent_harness_provider_openai::OpenAiCompatibleAdapter::new_async(
        config, std::sync::Arc::new(agent_harness_provider_openai::ReqwestTransport::new().unwrap())))
}
''')
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
    if args.with_provider:
        providers = [p for p in metadata["packages"] if p["name"] == "agent-harness-provider-openai"]
        if len(providers) != 1 or providers[0]["source"] is not None:
            raise SystemExit("Expected exactly one local provider.")
    print(f"Local host compatibility passed: one harness core at {core}.")
