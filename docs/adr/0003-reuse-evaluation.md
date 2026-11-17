# ADR 0003: bounded Gauntlet reuse evaluation

Date: 2026-11-11 · Status: provisional greenfield choice, F02 not fully closed.

Inspected source identity: `project-gauntlet/gauntlet` commit `ca066da5c9d56bd53a739c0dda1572fccc5019a8`, returned by GitHub's `commits/main` endpoint. No application binary was installed or run, and no Gauntlet source was copied into Ansa.

## Evidence

- [README at that commit](https://github.com/project-gauntlet/gauntlet/blob/ca066da5c9d56bd53a739c0dda1572fccc5019a8/README.md) explicitly says the project is no longer being developed. The commit introducing that notice is dated 2025-10-02. This describes the inspected upstream project, not every possible maintained fork.
- [Workspace manifest](https://github.com/project-gauntlet/gauntlet/blob/ca066da5c9d56bd53a739c0dda1572fccc5019a8/Cargo.toml) has Rust client/server/common/component-model/plugin-runtime/CLI/scenario-runner crates. Its renderer uses project-specific Iced/Iced-fonts branches. This is a substantial existing implementation, not merely a launcher UI sketch.
- The README documents a custom React reconciler, Deno plugins, application/window integration, aliases, shortcuts and frecency. It labels Windows and Wayland best-effort, while Linux X11 and Apple Silicon macOS are official targets. These are upstream documentation claims, not our validation results.
- [Plugin-runtime manifest](https://github.com/project-gauntlet/gauntlet/blob/ca066da5c9d56bd53a739c0dda1572fccc5019a8/rust/plugin_runtime/Cargo.toml) pins `deno_core 0.347.0` and `deno_runtime 0.213.0` and includes Numbat. The README's Deno-sandbox claim is not an independent security assessment; arbitrary Ansa Node compatibility must still be classified according to our own threat model.
- The [scenario-runner manifest](https://github.com/project-gauntlet/gauntlet/blob/ca066da5c9d56bd53a739c0dda1572fccc5019a8/rust/scenario_runner/Cargo.toml) wires a scenario feature into the client. Its presence does not establish passing conformance coverage. We did not build it or run its fixtures.
- [License](https://github.com/project-gauntlet/gauntlet/blob/ca066da5c9d56bd53a739c0dda1572fccc5019a8/LICENSE) is MPL-2.0. This does not prohibit reuse in a larger work, but source-file obligations and notices must be preserved. Ansa cannot copy its files and simply label them Apache-2.0. A per-file and dependency audit would precede reuse.

## Decision for this slice

Keep the small Ansa foundation rather than switch to a wholesale Gauntlet fork now. The inspected upstream has explicitly ended development, Windows is best-effort, and adopting its renderer forks/runtime would transfer a different maintenance surface before resolving Ansa's accessibility gate. There is no demonstrated active-upstream contribution route to assume. This is an engineering choice, not a claim that reimplementation is always cheaper or that MPL is unusable.

Revisit specific component/SDK ideas with license review when the extension platform starts. Do not implement Deno/Node compatibility merely by copying the README's architecture. Maintained forks, other candidate launchers, actual build/test quality, and migration cost remain uninvestigated; those keep the broader F02 ticket open.
