#!/usr/bin/env python3
"""Check a local Nausicaa consumer without editing either source tree."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--nausicaa", type=Path, required=True)
parser.add_argument("--with-provider", action="store_true", help="Also unify native provider and workflow composition")
parser.add_argument("--test-workspace", action="store_true", help="Run Ohmu's workspace tests with both local harness packages")
args = parser.parse_args()
with_provider = args.with_provider or args.test_workspace
root = Path(__file__).resolve().parents[1]
core = (args.nausicaa.resolve() / "crates/harness-core").resolve(strict=True)
provider = core.parent / "provider-openai"
environment = dict(os.environ, CARGO_TARGET_DIR=str(root / "target/local-host"))

# JSON strings are valid TOML basic strings for these filesystem paths. They
# are written as data, never interpolated into a shell command.
def quoted(path):
    return json.dumps(str(path), ensure_ascii=False)


def cargo(manifest, *arguments, capture=False):
    return subprocess.run(
        ["cargo", *arguments, "--manifest-path", str(manifest)],
        cwd=root, env=environment, check=True, text=True, stdout=subprocess.PIPE if capture else None,
    )


def check_packages(manifest):
    metadata = json.loads(cargo(
        manifest, "metadata", "--format-version", "1", "--locked", "--offline", capture=True,
    ).stdout)
    expected = {"agent-harness-core": core}
    if with_provider:
        expected["agent-harness-provider-openai"] = provider
    for name, directory in expected.items():
        packages = [package for package in metadata["packages"] if package["name"] == name]
        if (len(packages) != 1 or packages[0]["source"] is not None
                or Path(packages[0]["manifest_path"]).resolve() != directory.resolve() / "Cargo.toml"):
            raise SystemExit(f"Expected exactly one local {name} at {directory}.")


with tempfile.TemporaryDirectory(prefix="ohmu-local-host-") as directory:
    consumer = Path(directory)
    manifest = consumer / "Cargo.toml"
    patch = f'''[patch."https://github.com/Ceskoslov/Nausicaa.git"]
agent-harness-core = {{ path = {quoted(core)} }}
'''
    if with_provider:
        patch += f'agent-harness-provider-openai = {{ path = {quoted(provider)} }}\n'

    if args.test_workspace:
        shutil.copytree(root / "crates", consumer / "crates")
        shutil.copyfile(root / "Cargo.lock", consumer / "Cargo.lock")
        manifest.write_text((root / "Cargo.toml").read_text(encoding="utf-8") + "\n" + patch, encoding="utf-8")
        cargo(manifest, "update", "--offline", "-p", "agent-harness-core", "-p", "agent-harness-provider-openai")
        check_packages(manifest)
        cargo(manifest, "test", "--workspace", "--all-features", "--locked", "--offline")
        print(f"Ohmu workspace tests passed with local harness packages at {core.parent}.")
        sys.exit(0)

    (consumer / "src").mkdir()
    provider_dependency = f'agent-harness-provider-openai = {{ path = {quoted(provider)}, features = ["reqwest-transport"] }}\n' if with_provider else ''
    manifest.write_text(f'''[package]
name = "ohmu-local-host-check"
version = "0.0.0"
edition = "2024"

[workspace]

[dependencies]
agent-harness-core = {{ path = {quoted(core)} }}
ohmu-engine = {{ path = {quoted(root / "crates/ohmu-engine")} }}
ohmu-nausicaa = {{ path = {quoted(root / "crates/ohmu-nausicaa")} }}

{provider_dependency}
{patch}
''', encoding="utf-8")
    (consumer / "src/main.rs").write_text('''use std::sync::Arc;
use agent_harness_core::{RegistryError, ToolRegistry};
use ohmu_engine::Wiki;
fn main() {
    let _register: fn(&mut ToolRegistry, Arc<Wiki>) -> Result<(), RegistryError> =
        ohmu_nausicaa::register_read_tools;
}
''', encoding="utf-8")
    if with_provider:
        with (consumer / "src/main.rs").open("a") as source:
            source.write('''
#[allow(dead_code)]
fn provider_boundary(config: agent_harness_provider_openai::OpenAiConfig) -> std::sync::Arc<dyn agent_harness_core::ModelAdapter> {
    std::sync::Arc::new(agent_harness_provider_openai::OpenAiCompatibleAdapter::new_async(
        config, std::sync::Arc::new(agent_harness_provider_openai::ReqwestTransport::new().unwrap())))
}
''')
    cargo(manifest, "generate-lockfile", "--offline")
    cargo(manifest, "check", "--locked", "--offline")
    check_packages(manifest)
    print(f"Local host compatibility passed: one harness core at {core}.")
