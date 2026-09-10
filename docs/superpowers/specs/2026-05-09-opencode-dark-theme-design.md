# opencode-dark Theme Design

## Goal

Add a compact dark TUI theme for `xrun-tui` that feels close to opencode: terminal-first, low chrome, dark surfaces, crisp borders, and focused command/status accents.

## Scope

- Add a new selectable theme named `opencode-dark`.
- Keep the current theme architecture: render a palette-mapped stylesheet from `tokyo_night.tcss` into the user config directory.
- Do not redesign layouts or change screen structure.
- Preserve the existing immediate theme application behavior from Settings.

## Visual Direction

Use the approved "A. Dense Terminal" direction:

- Background: near-black blue/gray, darker than Tokyo Night.
- Surfaces: subtle dark panels with thin cool-gray borders.
- Text: high-contrast off-white for primary text, muted gray-blue for secondary text.
- Focus/active accent: clear terminal blue.
- Success/running: restrained green.
- Warning: amber.
- Error/destructive: red/pink.
- Spacing and density: compact; no soft card-heavy styling.

## Implementation Shape

The theme system already maps canonical Tokyo Night color tokens to alternate palettes. `opencode-dark` should be another palette entry in `xrun_tui.themes.PALETTES`.

Settings should expose the new option in `_THEMES` as `opencode-dark` with the label `OpenCode Dark`.

No new renderer, config file format, or compatibility layer is needed.

## Testing

Add/update Python TUI tests to verify:

- `render_theme("opencode-dark")` produces distinct CSS from Tokyo Night.
- The rendered CSS contains representative opencode-dark colors.
- The Settings theme list includes `opencode-dark`.

Run focused Python TUI tests with `python -m pytest tests/test_themes.py -q`, then the package test suite with `python -m pytest -q` from `python/xrun_tui`.

## Out Of Scope

- Building a full opencode clone theme with unrelated layout changes.
- Reworking all TCSS selectors.
- Adding light variants.
- Changing Rust TUI styling.
