# ANS-75: live frosted glass

The Windows launcher uses a host-backdrop brush and a Direct2D Gaussian blur on a
Windows Composition target below GPUI's swap chain. The 80% theme tint is painted
over the blur. Only the native compositor samples what's behind the window; no
screen capture or wallpaper processing is involved.

- The blur has a rounded clip sized in physical pixels to match the GPUI panel.
- Glass Off removes the backdrop from rendering; `--opaque` does not attach it
  at startup. Native setup failure and disabled transparency effects at setup
  keep the launcher opaque and report **Glass: Unavailable**.
- The palette test models source-over blending against black and white and
  checks at least 4.5:1 contrast for text and the caret. It does not measure the
  Windows compositor's output.

## Automated checks

```powershell
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo run --locked -p ansa-desktop -- --interaction-smoke-test --require-frost
cargo run --locked -p ansa-desktop -- --interaction-smoke-test --require-frost --light --opaque
```

`--require-frost` makes the debug interaction smoke fail if it exercises opaque
fallback instead of attaching the backdrop. The smoke toggles glass and theme,
and asserts that the window bounds reported by GPUI do not change. To test the fallback,
run the same smoke without `--require-frost` with
`GPUI_DISABLE_DIRECT_COMPOSITION=1` in the test process's environment.

## Visual acceptance (pending)

1. Open Ansa over another window containing broad, contrasting color blocks and
   fine text. With **Glass: On**, broad colors should bleed through with softened
   edges, the fine text behind Ansa should be indistinct, and Ansa's own text
   should remain crisp.
2. Change the underlying window's colors without changing wallpaper. The frost
   should follow the live scene. A uniformly colored panel or clearly visible
   underlying text is a failure even if the native setup smoke passed.
3. Toggle to opaque with `Ctrl+Shift+G` and back, then repeat in the light theme.
   Check the rounded silhouette, search focus, and that the window does not move
   or resize. Windows may suppress transparency under some system or power policies.
