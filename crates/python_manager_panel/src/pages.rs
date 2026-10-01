//! The five `SettingPage`s of the Python Package Manager tab: General,
//! Search, Installed, Updates, Vulnerabilities. Mirrors the Forge original's
//! `PythonManagerPanel::pages`/`installed_page`/`outdated_page`/
//! `vulnerabilities_page`/`count_badge` almost verbatim.

use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, Hsla, InteractiveElement as _, IntoElement,
    MouseButton, ParentElement as _, StatefulInteractiveElement as _, Styled as _, div,
    prelude::FluentBuilder as _, white,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, Icon,
    IconName::{self, Inbox, Redo2, Search, Settings2, TriangleAlert},
    Sizable as _, Size, StyledExt as _,
    button::{Button, ButtonVariants as _},
    collapsible::Collapsible,
    h_flex,
    input::Input,
    setting::{SettingGroup, SettingItem, SettingPage},
    spinner::Spinner,
    text::TextView,
    v_flex,
};
use python_backend::{PythonOutdatedPkg, PythonPackage, PyPiVulnerability};

use crate::{
    ActSelectedPackage, OpenSelectedPackage, PythonManagerPanel, SelectNextPackage,
    SelectPrevPackage,
};

/// The key context each package-list page's own container sets — see
/// `python_manager_panel::init`'s `cx.bind_keys` for the up/down/enter/space
/// bindings scoped to it, and why they live there rather than in the JSON
/// keymap (mirrors `gpui_component::table::data_table`'s own `DataTable`
/// context for the same reason; see also `npm_manager_panel`/
/// `nuget_manager_panel::pages`'s identical `PACKAGE_LIST_CONTEXT`).
const PACKAGE_LIST_CONTEXT: &str = "PythonPackageList";

/// Moves `selected` one row up (`forward: false`) or down (`forward: true`)
/// within a `len`-row list, wrapping at both ends — matches
/// `gpui_component::table::TableState`'s default `loop_selection` behavior
/// — and starting from the top row on the very first press.
fn step_selected(selected: Option<usize>, len: usize, forward: bool) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let ix = selected.unwrap_or(0);
    Some(if forward { (ix + 1) % len } else { (ix + len - 1) % len })
}

pub(crate) fn build_all(
    panel: &PythonManagerPanel,
    view: &Entity<PythonManagerPanel>,
    cx: &mut Context<PythonManagerPanel>,
) -> Vec<SettingPage> {
    let view = view.clone();
    let installed = panel.installed.clone();
    let outdated = panel.outdated.clone();
    let loading = panel.loading;
    let python_exe = panel.python_exe.clone();

    // ── General ────────────────────────────────────────────────────
    let mut general = SettingPage::new("General")
        .default_open(true)
        .icon(Icon::new(Settings2))
        .resettable(false)
        .group(
            SettingGroup::new().title("Interpreter").items(vec![
                SettingItem::render({
                    let view = view.clone();
                    move |_options, _window, cx| {
                        let exe = view.read(cx).python_exe.clone();
                        let using_venv = view.read(cx).using_venv;
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(if using_venv {
                                        "Scoped to this project's virtual environment:"
                                    } else {
                                        "No project venv found — falling back to the global interpreter:"
                                    }),
                            )
                            .child(
                                div()
                                    .font_family("Cascadia Mono")
                                    .text_xs()
                                    .text_color(cx.theme().foreground)
                                    .child(exe.unwrap_or_else(|| "No Python found".into())),
                            )
                    }
                }),
                SettingItem::render({
                    let view = view.clone();
                    move |_options, _window, cx| {
                        let running = view.read(cx).running_action.is_some();
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                Button::new("refresh")
                                    .icon(Redo2)
                                    .label("Refresh")
                                    .with_size(Size::Small)
                                    .disabled(running)
                                    .on_click({
                                        let view = view.clone();
                                        move |_, _, cx| {
                                            view.update(cx, |p, cx| {
                                                p.schedule_load(cx);
                                            });
                                        }
                                    }),
                            )
                            .when(running, |row| row.child(Spinner::new().xsmall()))
                    }
                })
                .description("Re-run pip list / pip list --outdated.")
                .keywords(["reload"]),
            ]),
        );

    if let Some(marker) = panel.can_create_venv {
        let view = view.clone();
        general = general.group(SettingGroup::new().title("Environment").item(SettingItem::render(
            move |_options, _window, cx| {
                let view = view.clone();
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!(
                                "This project has a {} but no virtual environment yet.",
                                marker.file_name()
                            )),
                    )
                    .child(
                        Button::new("create-venv")
                            .ghost()
                            .xsmall()
                            .label(format!("Create Environment from {}", marker.file_name()))
                            .on_click(move |_, window, cx| {
                                view.update(cx, |p, cx| {
                                    p.create_venv(window, cx);
                                });
                            }),
                    )
            },
        )));
    }

    // ── Search page ────────────────────────────────────────────────
    let search_page_item = {
        let view = view.clone();
        SettingItem::render(move |_options, _window, cx| {
            let view = view.clone();
            let search_input = view.read(cx).search_input.clone();
            h_flex()
                .gap_2()
                .items_center()
                .child(div().flex_1().min_w_0().child(Input::new(&search_input).xsmall()))
                .child(
                    Button::new("pypi-search-btn")
                        .ghost()
                        .xsmall()
                        .label("Look up")
                        .on_click({
                            let view = view.clone();
                            move |_, _, cx| {
                                view.update(cx, |p, cx| {
                                    p.search_pypi(cx);
                                });
                            }
                        }),
                )
        })
    };

    let search_results_item = {
        let view = view.clone();
        SettingItem::render(move |_options, window, cx| {
            let view = view.clone();
            let hit = view.read(cx).search_hit.clone();
            let search_loading = view.read(cx).search_loading;
            let search_error = view.read(cx).search_error.clone();
            let installed = view.read(cx).installed.clone();

            let content: AnyElement = if search_loading {
                h_flex()
                    .gap_2()
                    .items_center()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(Spinner::new().xsmall())
                    .child("Looking up on PyPI\u{2026}")
                    .into_any_element()
            } else if let Some(err) = &search_error {
                div()
                    .text_xs()
                    .text_color(cx.theme().danger)
                    .child(err.clone())
                    .into_any_element()
            } else if let Some(hit) = &hit {
                let name = hit.name.clone();
                let is_installed = installed.iter().any(|p| p.name == name);
                h_flex()
                    .gap_2()
                    .items_center()
                    .w_full()
                    .py_1()
                    .border_b_1()
                    .border_color(cx.theme().border.opacity(0.3))
                    .child(
                        div()
                            .id(format!("search-hit-{name}"))
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family("Cascadia Mono")
                            .text_xs()
                            .font_medium()
                            .text_color(cx.theme().foreground)
                            .child(name.clone())
                            .cursor_pointer()
                            .hover(|d| d.text_color(cx.theme().primary))
                            .on_click({
                                let view = view.clone();
                                move |_, _, cx| {
                                    view.update(cx, |p, cx| {
                                        p.open_search_hit_details(cx);
                                    });
                                }
                            }),
                    )
                    .child(
                        div()
                            .font_family("Cascadia Mono")
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(hit.version.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(hit.summary.clone().unwrap_or_default()),
                    )
                    .child(if is_installed {
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Installed")
                            .into_any_element()
                    } else {
                        let version = hit.version.clone();
                        Button::new(format!("install-{name}"))
                            .ghost()
                            .xsmall()
                            .label("Install")
                            .on_click({
                                let view = view.clone();
                                let name = name.clone();
                                move |_, window, cx| {
                                    view.update(cx, |p, cx| {
                                        p.selected = Some(name.clone());
                                        p.install_pkg(&name, Some(&version), window, cx);
                                    });
                                }
                            })
                            .into_any_element()
                    })
                    .into_any_element()
            } else {
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Look up an exact package name on PyPI (e.g. \"requests\")")
                    .into_any_element()
            };

            let _ = window;
            v_flex().gap_2().w_full().child(content)
        })
    };

    let search = SettingPage::new("Search").icon(Icon::new(Search)).resettable(false).group(
        SettingGroup::new()
            .title("pypi.org")
            .item(search_page_item)
            .item(search_results_item),
    );

    // ── Installed / Updates pages ──────────────────────────────────
    let installed_page = build_installed_page(
        &installed,
        &view,
        loading,
        python_exe.is_some(),
        panel.installed_cursor,
        panel.installed_list_focus.clone(),
    );
    let outdated_page = build_outdated_page(
        &outdated,
        &view,
        panel.outdated_cursor,
        panel.outdated_list_focus.clone(),
    );

    // ── Vulnerabilities page ────────────────────────────────────────
    let vulnerabilities_page = build_vulnerabilities_page(
        &panel.vulns,
        panel.vuln_scanning,
        panel.vuln_scanned,
        panel.vuln_scan_error.clone(),
        &view,
        panel.vuln_cursor,
        panel.vuln_list_focus.clone(),
    );

    let _ = cx;
    vec![general, search, installed_page, outdated_page, vulnerabilities_page]
}

fn build_installed_page(
    installed: &[PythonPackage],
    view: &Entity<PythonManagerPanel>,
    loading: bool,
    has_python: bool,
    cursor: Option<usize>,
    focus_handle: FocusHandle,
) -> SettingPage {
    let mut group = SettingGroup::new().title("Installed Packages");
    if installed.is_empty() && !loading {
        group = group.item(SettingItem::render(move |_o, _w, cx| {
            v_flex().gap_1().text_xs().text_color(cx.theme().muted_foreground).child(if has_python {
                "No packages installed."
            } else {
                "No Python interpreter found in PATH."
            })
        }));
    } else {
        let rows = installed.to_vec();
        let view = view.clone();
        group = group.item(SettingItem::render(move |_o, _w, cx| {
            let view = view.clone();
            let len = rows.len();
            let mut list = v_flex()
                .id("py-installed-list")
                .track_focus(&focus_handle)
                .on_mouse_down(MouseButton::Left, {
                    let focus_handle = focus_handle.clone();
                    move |_, window, cx| {
                        window.focus(&focus_handle, cx);
                    }
                })
                .key_context(PACKAGE_LIST_CONTEXT)
                .on_action({
                    let view = view.clone();
                    move |_: &SelectNextPackage, _window, cx: &mut App| {
                        view.update(cx, |p, cx| {
                            p.installed_cursor = step_selected(p.installed_cursor, len, true);
                            cx.notify();
                        });
                    }
                })
                .on_action({
                    let view = view.clone();
                    move |_: &SelectPrevPackage, _window, cx: &mut App| {
                        view.update(cx, |p, cx| {
                            p.installed_cursor = step_selected(p.installed_cursor, len, false);
                            cx.notify();
                        });
                    }
                })
                .on_action({
                    let view = view.clone();
                    let rows = rows.clone();
                    move |_: &OpenSelectedPackage, _window, cx: &mut App| {
                        view.update(cx, |p, cx| {
                            let Some(name) = p.installed_cursor.and_then(|ix| rows.get(ix)).map(|pkg| pkg.name.clone()) else {
                                return;
                            };
                            p.fetch_details(name, cx);
                        });
                    }
                })
                .on_action({
                    let view = view.clone();
                    let rows = rows.clone();
                    move |_: &ActSelectedPackage, window, cx: &mut App| {
                        view.update(cx, |p, cx| {
                            let Some(name) = p.installed_cursor.and_then(|ix| rows.get(ix)).map(|pkg| pkg.name.clone()) else {
                                return;
                            };
                            p.selected = Some(name.clone());
                            p.uninstall_pkg(&name, window, cx);
                        });
                    }
                })
                .gap_1p5();

            for (ix, pkg) in rows.iter().enumerate() {
                let name = pkg.name.clone();
                let version = pkg.version.clone();
                let view = view.clone();
                let is_selected = cursor == Some(ix);

                let row = h_flex()
                    .id(("py-installed-row", ix))
                    .gap_2()
                    .items_center()
                    .w_full()
                    .rounded_md()
                    .when(is_selected, |row| {
                        row.bg(cx.theme().primary.opacity(0.08)).border_1().border_color(cx.theme().primary)
                    })
                    .on_click({
                        let view = view.clone();
                        move |_, _, cx| {
                            view.update(cx, |p, cx| {
                                p.installed_cursor = Some(ix);
                                cx.notify();
                            });
                        }
                    })
                    .child(
                        div()
                            .id(format!("inst-{name}"))
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family("Cascadia Mono")
                            .text_xs()
                            .text_color(cx.theme().foreground)
                            .cursor_pointer()
                            .hover(|d| d.text_color(cx.theme().primary))
                            .child(name.clone())
                            .on_click({
                                let name = name.clone();
                                let view = view.clone();
                                move |_, _, cx| {
                                    view.update(cx, |p, cx| {
                                        p.fetch_details(name.clone(), cx);
                                    });
                                }
                            }),
                    )
                    .child(
                        div()
                            .font_family("Cascadia Mono")
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(version.clone()),
                    )
                    .when_some(view.read(cx).latest_for(&name).map(str::to_string), |row, latest| {
                        if latest != version {
                            row.child(
                                Button::new(format!("quick-update-{name}"))
                                    .ghost()
                                    .xsmall()
                                    .label(format!("Update \u{2192} {latest}"))
                                    .on_click({
                                        let name = name.clone();
                                        let latest = latest.clone();
                                        let view = view.clone();
                                        move |_, window, cx| {
                                            view.update(cx, |p, cx| {
                                                p.selected = Some(name.clone());
                                                p.install_pkg(&name, Some(&latest), window, cx);
                                            });
                                        }
                                    }),
                            )
                        } else {
                            row
                        }
                    })
                    .child(
                        Button::new(format!("uninstall-{name}"))
                            .ghost()
                            .xsmall()
                            .label("Uninstall")
                            .on_click({
                                let name = name.clone();
                                let view = view.clone();
                                move |_, window, cx| {
                                    view.update(cx, |p, cx| {
                                        p.selected = Some(name.clone());
                                        p.uninstall_pkg(&name, window, cx);
                                    });
                                }
                            }),
                    );
                list = list.child(row);
            }
            list
        }));
    }
    let count = installed.len();
    SettingPage::new("Installed")
        .icon(Icon::new(Inbox))
        .resettable(false)
        .group(group)
        .sidebar_badge(move |_, cx| count_badge(count, loading, cx))
}

fn build_outdated_page(
    outdated: &[PythonOutdatedPkg],
    view: &Entity<PythonManagerPanel>,
    cursor: Option<usize>,
    focus_handle: FocusHandle,
) -> SettingPage {
    let mut group = SettingGroup::new().title("Outdated Packages");
    if outdated.is_empty() {
        group = group.item(SettingItem::render(move |_o, _w, cx| {
            v_flex()
                .gap_1()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child("All packages are up to date \u{2713}")
        }));
    } else {
        let view_for_bulk = view.clone();
        group = group.item(SettingItem::render(move |_o, _window, _cx| {
            let view = view_for_bulk.clone();
            Button::new("update-all").ghost().xsmall().label("Update All").on_click(move |_, window, cx| {
                view.update(cx, |p, cx| {
                    p.update_all_outdated(window, cx);
                });
            })
        }));

        let rows = outdated.to_vec();
        let view = view.clone();
        group = group.item(SettingItem::render(move |_o, _w, cx| {
            let view = view.clone();
            let len = rows.len();
            let mut list = v_flex()
                .id("py-outdated-list")
                .track_focus(&focus_handle)
                .on_mouse_down(MouseButton::Left, {
                    let focus_handle = focus_handle.clone();
                    move |_, window, cx| {
                        window.focus(&focus_handle, cx);
                    }
                })
                .key_context(PACKAGE_LIST_CONTEXT)
                .on_action({
                    let view = view.clone();
                    move |_: &SelectNextPackage, _window, cx: &mut App| {
                        view.update(cx, |p, cx| {
                            p.outdated_cursor = step_selected(p.outdated_cursor, len, true);
                            cx.notify();
                        });
                    }
                })
                .on_action({
                    let view = view.clone();
                    move |_: &SelectPrevPackage, _window, cx: &mut App| {
                        view.update(cx, |p, cx| {
                            p.outdated_cursor = step_selected(p.outdated_cursor, len, false);
                            cx.notify();
                        });
                    }
                })
                .on_action({
                    let view = view.clone();
                    let rows = rows.clone();
                    move |_: &OpenSelectedPackage, _window, cx: &mut App| {
                        view.update(cx, |p, cx| {
                            let Some(name) = p.outdated_cursor.and_then(|ix| rows.get(ix)).map(|pkg| pkg.name.clone()) else {
                                return;
                            };
                            p.fetch_details(name, cx);
                        });
                    }
                })
                .on_action({
                    let view = view.clone();
                    let rows = rows.clone();
                    move |_: &ActSelectedPackage, window, cx: &mut App| {
                        view.update(cx, |p, cx| {
                            let Some((name, latest)) = p
                                .outdated_cursor
                                .and_then(|ix| rows.get(ix))
                                .map(|pkg| (pkg.name.clone(), pkg.latest_version.clone()))
                            else {
                                return;
                            };
                            p.selected = Some(name.clone());
                            p.install_pkg(&name, Some(&latest), window, cx);
                        });
                    }
                })
                .gap_1p5();

            for (ix, pkg) in rows.iter().enumerate() {
                let name = pkg.name.clone();
                let current = pkg.version.clone();
                let latest = pkg.latest_version.clone();
                let view = view.clone();
                let is_selected = cursor == Some(ix);

                let row = h_flex()
                    .id(("py-outdated-row", ix))
                    .gap_2()
                    .items_center()
                    .w_full()
                    .rounded_md()
                    .when(is_selected, |row| {
                        row.bg(cx.theme().primary.opacity(0.08)).border_1().border_color(cx.theme().primary)
                    })
                    .on_click({
                        let view = view.clone();
                        move |_, _, cx| {
                            view.update(cx, |p, cx| {
                                p.outdated_cursor = Some(ix);
                                cx.notify();
                            });
                        }
                    })
                    .child(
                        div()
                            .id(format!("outdated-{name}"))
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family("Cascadia Mono")
                            .text_xs()
                            .text_color(cx.theme().foreground)
                            .cursor_pointer()
                            .hover(|d| d.text_color(cx.theme().primary))
                            .child(name.clone())
                            .on_click({
                                let name = name.clone();
                                let view = view.clone();
                                move |_, _, cx| {
                                    view.update(cx, |p, cx| {
                                        p.fetch_details(name.clone(), cx);
                                    });
                                }
                            }),
                    )
                    .child(
                        div()
                            .font_family("Cascadia Mono")
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{current} \u{2192} {latest}")),
                    )
                    .child(
                        Button::new(format!("update-{name}")).ghost().xsmall().label("Update").on_click({
                            let name = name.clone();
                            let latest = latest.clone();
                            let view = view.clone();
                            move |_, window, cx| {
                                view.update(cx, |p, cx| {
                                    p.selected = Some(name.clone());
                                    p.install_pkg(&name, Some(&latest), window, cx);
                                });
                            }
                        }),
                    );
                list = list.child(row);
            }
            list
        }));
    }
    let count = outdated.len();
    SettingPage::new("Updates")
        .icon(Icon::new(Redo2))
        .resettable(false)
        .group(group)
        .sidebar_badge(move |_, cx| count_badge(count, false, cx))
}

fn build_vulnerabilities_page(
    vulns: &std::collections::HashMap<String, Vec<PyPiVulnerability>>,
    scanning: bool,
    scanned: bool,
    error: Option<String>,
    view: &Entity<PythonManagerPanel>,
    cursor: Option<usize>,
    focus_handle: FocusHandle,
) -> SettingPage {
    let mut group = SettingGroup::new().title("Known Vulnerabilities (PyPI / OSV.dev)");

    {
        let view = view.clone();
        group = group
            .item(
                SettingItem::render(move |_o, _w, _cx| {
                    let view = view.clone();
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Button::new("scan-vulns")
                                .ghost()
                                .xsmall()
                                .label(if scanning { "Scanning\u{2026}" } else { "Scan Installed Packages" })
                                .disabled(scanning)
                                .on_click(move |_, _, cx| {
                                    view.update(cx, |p, cx| {
                                        p.scan_vulnerabilities(cx);
                                    });
                                }),
                        )
                        .when(scanning, |row| row.child(Spinner::new().xsmall()))
                })
                .description(
                    "Checks each installed package's exact version against OSV-sourced advisory \
                     data — one PyPI request per package, so this runs on demand rather than \
                     automatically.",
                ),
            );
    }

    if let Some(err) = &error {
        let err = err.clone();
        group = group.item(SettingItem::render(move |_o, _w, cx| {
            div().text_xs().text_color(cx.theme().danger).child(err.clone())
        }));
    } else if !scanned {
        group = group.item(SettingItem::render(move |_o, _w, cx| {
            div().text_xs().text_color(cx.theme().muted_foreground).child("Not scanned yet.")
        }));
    } else if vulns.is_empty() {
        group = group.item(SettingItem::render(move |_o, _w, cx| {
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child("No known vulnerabilities found in installed packages \u{2713}")
        }));
    } else {
        // Flattened, name-sorted so display order and `vuln_cursor`'s index
        // agree — same order the old per-row double loop rendered in.
        let mut names: Vec<&String> = vulns.keys().collect();
        names.sort();
        let rows: Vec<(String, PyPiVulnerability)> = names
            .into_iter()
            .filter_map(|name| vulns.get(name).map(|vs| (name, vs)))
            .flat_map(|(name, vs)| vs.iter().map(move |v| (name.clone(), v.clone())))
            .collect();

        let view = view.clone();
        group = group.item(SettingItem::render(move |_o, _w, cx| {
            let view = view.clone();
            let len = rows.len();
            let mut list = v_flex()
                .id("py-vuln-list")
                .track_focus(&focus_handle)
                .on_mouse_down(MouseButton::Left, {
                    let focus_handle = focus_handle.clone();
                    move |_, window, cx| {
                        window.focus(&focus_handle, cx);
                    }
                })
                .key_context(PACKAGE_LIST_CONTEXT)
                .on_action({
                    let view = view.clone();
                    move |_: &SelectNextPackage, _window, cx: &mut App| {
                        view.update(cx, |p, cx| {
                            p.vuln_cursor = step_selected(p.vuln_cursor, len, true);
                            cx.notify();
                        });
                    }
                })
                .on_action({
                    let view = view.clone();
                    move |_: &SelectPrevPackage, _window, cx: &mut App| {
                        view.update(cx, |p, cx| {
                            p.vuln_cursor = step_selected(p.vuln_cursor, len, false);
                            cx.notify();
                        });
                    }
                })
                .on_action({
                    let view = view.clone();
                    let rows = rows.clone();
                    move |_: &OpenSelectedPackage, _window, cx: &mut App| {
                        view.update(cx, |p, cx| {
                            let Some((pkg_name, v)) = p.vuln_cursor.and_then(|ix| rows.get(ix)) else {
                                return;
                            };
                            let vuln_id = format!("{pkg_name}-{}", v.id);
                            p.toggle_vuln(vuln_id, cx);
                        });
                    }
                })
                .on_action({
                    let view = view.clone();
                    let rows = rows.clone();
                    move |_: &ActSelectedPackage, window, cx: &mut App| {
                        view.update(cx, |p, cx| {
                            let Some((name, fix)) = p.vuln_cursor.and_then(|ix| rows.get(ix)).and_then(
                                |(pkg_name, v)| Some((pkg_name.clone(), v.fixed_in.first()?.clone())),
                            ) else {
                                return;
                            };
                            p.selected = Some(name.clone());
                            p.install_pkg(&name, Some(&fix), window, cx);
                        });
                    }
                })
                .gap_1p5();

            for (ix, (pkg_name, v)) in rows.iter().enumerate() {
                let pkg_name = pkg_name.clone();
                let id = v.id.clone();
                let text = v.summary.clone().or_else(|| v.details.clone()).unwrap_or_default();
                let fixed_in = v.fixed_in.clone();
                let view = view.clone();
                let vuln_id = format!("{pkg_name}-{id}");
                let can_expand = !text.is_empty();
                let is_open = can_expand && view.read(cx).expanded_vulns.contains(&vuln_id);
                let is_selected = cursor == Some(ix);
                let vuln_id_for_click = vuln_id.clone();
                let view_for_click = view.clone();

                let header = h_flex()
                    .id(format!("vuln-toggle-{vuln_id}"))
                    .gap_2()
                    .items_center()
                    .w_full()
                    .when(can_expand, |row| {
                        row.cursor_pointer()
                            .hover(|d| d.bg(cx.theme().list_hover))
                            .on_click(move |_, _, cx| {
                                view_for_click.update(cx, |p, cx| {
                                    p.toggle_vuln(vuln_id_for_click.clone(), cx);
                                });
                            })
                            .child(
                                Icon::new(if is_open { IconName::ChevronDown } else { IconName::ChevronRight })
                                    .xsmall()
                                    .text_color(cx.theme().muted_foreground),
                            )
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family("Cascadia Mono")
                            .text_xs()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(cx.theme().foreground)
                            .child(pkg_name.clone()),
                    )
                    .child(div().px_1().rounded_sm().text_xs().text_color(cx.theme().danger).child(id.clone()))
                    .when_some(fixed_in.first().cloned(), |row, fix: String| {
                        let name = pkg_name.clone();
                        let fix_id = id.clone();
                        let view = view.clone();
                        row.child(
                            Button::new(format!("fix-{name}-{fix_id}"))
                                .ghost()
                                .xsmall()
                                .label(format!("Update \u{2192} {fix}"))
                                .on_click(move |_, window, cx| {
                                    cx.stop_propagation();
                                    view.update(cx, |p, cx| {
                                        p.selected = Some(name.clone());
                                        p.install_pkg(&name, Some(&fix), window, cx);
                                    });
                                }),
                        )
                    });

                let row_click_view = view.clone();
                let row_wrap = v_flex()
                    .id(("py-vuln-row", ix))
                    .w_full()
                    .min_w_0()
                    .pb_2()
                    .rounded_md()
                    .when(is_selected, |row| {
                        row.bg(cx.theme().primary.opacity(0.08)).border_1().border_color(cx.theme().primary)
                    })
                    .on_click(move |_, _, cx| {
                        row_click_view.update(cx, |p, cx| {
                            p.vuln_cursor = Some(ix);
                            cx.notify();
                        });
                    });

                let row_wrap = if !can_expand {
                    row_wrap.child(header)
                } else {
                    let body = v_flex().w_full().min_w_0().gap_1().pl_2().child(
                        div().max_w(gpui::px(900.)).min_w_0().text_xs().text_color(cx.theme().muted_foreground).child(
                            TextView::markdown(format!("vuln-summary-{pkg_name}-{id}"), text.clone())
                                .w_full()
                                .min_w_0()
                                .selectable(true),
                        ),
                    );
                    row_wrap.child(Collapsible::new().w_full().min_w_0().open(is_open).child(header).content(body))
                };
                list = list.child(row_wrap);
            }
            list
        }));
    }

    let count: usize = vulns.values().map(|v| v.len()).sum();
    SettingPage::new("Vulnerabilities")
        .icon(Icon::new(TriangleAlert))
        .resettable(false)
        .group(group)
        .sidebar_badge(move |_, cx| colored_count_badge(count, scanning, cx.theme().danger))
}

/// Sidebar count badge — `.flex()` before `.items_center().justify_center()`
/// is required or the count text won't vertically center (a bug both
/// `npm_manager_panel`/`nuget_manager_panel` had and had fixed).
fn count_badge(count: usize, loading: bool, cx: &App) -> AnyElement {
    colored_count_badge(count, loading, cx.theme().primary)
}

/// Like [`count_badge`], but with the pill's fill color set by the caller —
/// used for the Vulnerabilities badge, which needs the error color rather
/// than the primary-theme color the Installed/Updates badges use.
fn colored_count_badge(count: usize, loading: bool, color: Hsla) -> AnyElement {
    h_flex().child(
        div()
            .when(count == 0 && !loading, |this| this.size_0())
            .when(count > 0, |this| {
                this.flex()
                    .bg(color)
                    .rounded_full()
                    .px_1p5()
                    .min_w_3p5()
                    .h_4()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .text_color(white())
                    .child(count.to_string())
            })
            .when(count == 0 && loading, |this| this.child(Spinner::new().with_size(Size::XSmall))),
    )
    .into_any_element()
}
