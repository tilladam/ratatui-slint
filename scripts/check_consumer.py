#!/usr/bin/env python3
"""Build the public API and README application from a separate Cargo workspace.

Use --root for a verified package checkout, --toolchain 1.92 for fresh MSRV
resolution, or --check-only when execution is unavailable.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=Path(__file__).resolve().parent.parent)
    parser.add_argument('--toolchain')
    parser.add_argument('--check-only', action='store_true')
    args = parser.parse_args()
    root = args.root.resolve()
    env = dict(os.environ)
    env.setdefault('CARGO_TARGET_DIR', str(root / 'target'))
    cargo = ['cargo'] + ([f'+{args.toolchain}'] if args.toolchain else [])
    with tempfile.TemporaryDirectory(prefix='ratatui-slint-consumer-') as directory:
        work = Path(directory)
        (work / 'src').mkdir()
        (work / 'helper/src').mkdir(parents=True)
        lib = f'ratatui-slint = {{ path = {json.dumps(str(root))}, default-features = false }}\n'
        (work / 'Cargo.toml').write_text('''[package]
name = "release-consumer"
version = "0.0.0"
edition = "2024"
rust-version = "1.92"
[workspace]
[features]
default = ["ratatui-slint/default"]
transparency = ["ratatui-slint/transparency"]
unified = ["dep:alpha-helper"]
[dependencies]
slint = { version = "1.18", default-features = false, features = ["std", "compat-1-2"] }
ratatui = "0.30"
crossterm = "0.29"
base64 = "0.22"
image = { version = "0.25", default-features = false, features = ["png"] }
alpha-helper = { path = "helper", optional = true }
''' + lib)
        (work / 'helper/Cargo.toml').write_text('''[package]
name = "alpha-helper"
version = "0.0.0"
edition = "2024"
[dependencies]
''' + lib.rstrip().replace('false }', 'false, features = ["transparency"] }') + '\n')
        (work / 'helper/src/lib.rs').write_text('pub fn helper() {}\n')
        (work / 'src/main.rs').write_text((root / 'tests/consumer.rs.txt').read_text())
        command = 'check' if args.check_only else 'run'
        for features, alpha in [([], False), (['--no-default-features'], False),
                                (['--no-default-features', '--features', 'transparency'], True),
                                (['--no-default-features', '--features', 'unified'], True),
                                (['--all-features'], True)]:
            run_env = dict(env)
            run_env.pop('EXPECT_ALPHA', None)
            if alpha:
                run_env['EXPECT_ALPHA'] = '1'
            print(f'Consumer: {" ".join(features) or "default"}', flush=True)
            subprocess.run(cargo + [command, '--quiet'] + features, cwd=work, env=run_env,
                           check=True, timeout=1200)
        readme = (root / 'README.md').read_text()
        example = readme.split('<!-- consumer-example -->', 1)[1].split('```rust,no_run\n', 1)[1].split('\n```', 1)[0]
        (work / 'src/main.rs').write_text(example + '\n')
        subprocess.run(cargo + ['check', '--quiet'], cwd=work, env=env, check=True, timeout=1200)
    print('Consumer API, feature unification, output policy, and README checks passed.')


if __name__ == '__main__':
    main()
