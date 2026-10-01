.rules

# Cockpit panel ports: `gpui_component` is the requirement, not an option

Panels ported from the Forge repo (`E:\Forge_GPUI`) into this tree
(`cockpit_panel`, `script_runner_panel`, `processes_panel`, `python_panel`,
`node_panel`, `npm_manager_panel`) are expected to be built on
**`gpui_component`'s actual widget and layout system** —
`gpui_component::setting::{Settings, SettingPage, SettingGroup, SettingItem}`,
`h_resizable`/`resizable_panel`, `gpui_component::text::markdown`,
`Input`/`InputState`, etc. A port that renders flat, hand-rolled `div` lists
instead of these widgets is a **failed port**, even if it compiles and works.

The first version of `npm_manager_panel` shipped exactly that way (a
hand-rolled three-tab list, no `setting::`, no resizable split, no registry
search). The approved remedy lives in
`crates/gpui_component/NPM_MANAGER_PANEL_UI_RESTORE_PLAN.md` — read it before
touching the npm manager, and follow `crates/gpui_component/PORTING.md` for the
general porting conventions (crate layout, `AppState`-stripping, theme
bridging). Before declaring any such port done, confirm the UI is actually
composed from `gpui_component` widgets, not just `div`s that resemble them.