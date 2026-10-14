# Working in this game starter

- Use native idiomatic Rust/Bevy; no MDX or emulator runtime dependency is present.
- Keep gameplay systems in `FixedUpdate` / `GameSystems::Game`; that is the
  simulation boundary used by developer frame stepping.
- Use `ControllerState` and `Button` for gameplay input; edit device mappings in
  `controller.rs`. Use layer0 for game rendering and reserve layer1 for display.
- Read README.md for MCP startup, screenshot targets and input lifetime semantics.
- Build the shipping version without `dev-mcp`; keep all debug transport hooks
  in the feature-gated developer module.
- Verify both ordinary and `dev-mcp` builds when changing shell integration.
