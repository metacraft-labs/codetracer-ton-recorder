## Reprobuild dev env + build recipe for codetracer-ton-recorder.
##
## Mirrors the dev shell declared in ``flake.nix`` (Linux/macOS) and
## the Windows DIY env declared in ``env.ps1``. ``repro build`` /
## ``repro test`` reproduce the same artefacts and the same test set
## that ``just build`` / ``just test`` produce today.
##
## Per ``codetracer-specs/Repo-Requirements.md`` §2.8 the recipe
## expresses build and test execution NATIVELY through typed-tool
## edges (`cargo.build`, `cargo.test`). It does NOT delegate the Rust
## build / test to `shell(command = "bash scripts/...")` wrappers —
## delegation defeats the engine's incremental-build, action-cache,
## per-test invalidation, and the CI sharding the engine grows into per
## ``reprobuild-specs/CI-Sharding.md``. The ONE ``sh.shell`` edge below
## wraps the repo's CLI-convention verification script, which is not a
## cargo target — it is a POSIX-shell assertion harness that ``just
## test`` runs after ``cargo test`` (see ``Justfile`` ``test:``), so it
## is modelled as its own execute edge rather than dropped.
##
## On Windows the recipe drives real reprobuild tool provisioning via
## the tarball entries the ``uses:`` packages declare (cargo, rustc,
## rustfmt, nim, nimble, capnp). On Linux/macOS the Nix flake
## continues to supply the same toolchain. Either path produces
## byte-equivalent build outputs and the same test pass/fail set —
## CI cross-checks this through the side-by-side `ci.yml` (nix) +
## `ci-reprobuild.yml` (reprobuild) flow per Repo-Requirements §2.9.
##
## TON: test corpus is pre-compiled TVM .boc fixtures.

import repro_project_dsl
import repro_dsl_stdlib/packages/sh

package codetracer_ton_recorder:
  # Declare ``path``-mode tool provisioning so the engine adopts it
  # automatically. The nix dev shell puts cargo / rustc / nim / nimble /
  # capnp / zstd on PATH (and pkg-config / openssl on Linux/macOS), so
  # the weak-local PATH resolver is the right default. Without it
  # ``repro build`` refuses to run with "typed tool provisioning is
  # required for uses declarations".
  defaultToolProvisioning "path"

  uses:
    # Rust toolchain — declared by version so the tarball-direct
    # provisioning entries in repro_dsl_stdlib/packages/cargo.nim /
    # rustc.nim / rustfmt.nim resolve on Windows. On Linux/macOS the
    # nix flake supplies the same versions.
    "rustc >=1.85"
    "cargo >=1.85"

    # Nim toolchain — codetracer_trace_writer_nim's build.rs compiles
    # a static library at cargo build time.
    "nim >=2.2 <3.0"
    "nimble"

    # Cap'n Proto schema compiler used by the sibling trace-format
    # crates' build.rs (capnpc over the trace schema) at cargo build
    # time. The recorder itself has no build.rs.
    "capnp"

    # libzstd headers + library, needed when linking the Nim FFI
    # static library into the cargo build.
    "zstd"

    # pkg-config + OpenSSL — openssl-sys consults pkg-config to find
    # OpenSSL on Linux/macOS. The Windows build uses the rustls-tls
    # feature instead so neither is on the windows toolchain floor.
    when not defined(windows):
      "pkg-config"
      "openssl"

    # POSIX shell — drives the CLI-convention verification edge below,
    # the same ``bash tests/verify-cli-convention-no-silent-skip.sh``
    # step ``just test`` runs after ``cargo test``.
    "sh"

  executable codetracerTonRecorder:
    name: "codetracer-ton-recorder"

  devEnv:
    activity "default"

  build:
    # ---- Primary build edge (the `default` collection) ----------------
    #
    # Native cargo build for the recorder binary. Enrolled into the
    # conventional ``default`` collection per
    # reprobuild-specs/Build-Graph-Collections.md §"`default`"; this
    # makes ``repro build`` (no positional target) materialise this
    # edge's closure.
    const binarySuffix = (when defined(windows): ".exe" else: "")
    const recorderBinary =
      "target/release/codetracer-ton-recorder" & binarySuffix

    let recorderBuild = cargo.build(
      locked = true,
      release = true,
      actionId = "codetracer-ton-recorder.cargo-build",
      extraInputs = @[
        "Cargo.toml", "Cargo.lock",
        "src"
      ],
      extraOutputs = @[recorderBinary])
    discard collect("default", @[recorderBuild])

    # ---- Test-binary build + run edges (the `test` collection) -------
    #
    # Two-stage shape per Repo-Requirements.md §2.8: `cargo.test(noRun =
    # true)` builds every cargo test binary into
    # `target/debug/deps/<crate>-<hash>` (the engine tracks the deps
    # directory as the build edge's effect set because the hashed
    # filename floats with input content); `cargo.test(noRun = false)`
    # then runs the binaries in one cargo invocation. The execute edge
    # depends on the build edge so the engine only re-runs tests when
    # an input changed since the last successful execution.
    #
    # Per-test execute edges fall out automatically once the
    # ct-test-runner cargo adapter lands per
    # reprobuild-specs/Test-Edges-And-Parallel-Runner.milestones.org
    # §M4 — the whole-binary edge becomes a fan-out point without
    # changing this recipe.

    let testsBuild = cargo.test(
      locked = true,
      noRun = true,
      actionId = "codetracer-ton-recorder.cargo-test-build",
      extraInputs = @[
        "Cargo.toml", "Cargo.lock",
        "src", "tests"
      ],
      extraOutputs = @["target/debug/deps"])

    let testsRun = cargo.test(
      locked = true,
      actionId = "codetracer-ton-recorder.cargo-test-run",
      after = @[testsBuild.action],
      extraInputs = @[
        "Cargo.toml", "Cargo.lock",
        "src", "tests",
        "target/debug/deps"
      ])

    # ---- CLI-convention verification edge -----------------------------
    #
    # ``just test`` runs ``bash
    # tests/verify-cli-convention-no-silent-skip.sh`` after ``cargo
    # test``. The script asserts the recorder's ``--help`` / ``--version``
    # surface complies with ``Recorder-CLI-Conventions.md`` (no
    # ``--format`` leak, ``--out-dir`` / ``ct print`` present, the two
    # ``CODETRACER_TON_RECORDER_*`` env-var fallbacks referenced in
    # source). It is not a cargo target, so it is modelled as its own
    # ``sh.shell`` execute edge rather than dropped — reproducing the
    # repo's full ``just test`` set. The script itself does ``cargo build
    # --locked --quiet`` (a no-op once the recorder is built), then runs
    # the freshly-built debug binary via ``cargo run``; ``after`` the
    # cargo test-build edge guarantees the crate is compiled before the
    # script runs. Non-cacheable: the script inspects a runtime binary
    # via automatic monitoring and asserts on ``--help`` text, so it is
    # re-run every ``repro test`` pass (matching ``just test``).
    let cliVerify = shell(
      command = "bash tests/verify-cli-convention-no-silent-skip.sh",
      actionId = "codetracer-ton-recorder.verify-cli-convention",
      after = @[testsBuild.action],
      extraInputs = @[
        "tests/verify-cli-convention-no-silent-skip.sh",
        "Cargo.toml", "Cargo.lock", "src"
      ],
      cacheable = false)

    discard collect("test", @[testsRun.action, cliVerify])
