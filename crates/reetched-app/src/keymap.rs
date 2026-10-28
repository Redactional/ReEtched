//! Keyboard shortcuts: named actions bound to keys, with Painter-style and
//! Blender-style presets and per-user overrides (saved in Settings).

use egui::{Key, Modifiers};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub enum Action {
    NewProject,
    OpenProject,
    Save,
    SaveAs,
    ExportTextures,
    Undo,
    Redo,
    Delete,
    FrameView,
    BrushSmaller,
    BrushBigger,
    ToolPaint,
    ToolEraser,
    ToolProjection,
    ToolPolygonFill,
    ToolSmudge,
    ToolClone,
    ToolPath,
}

impl Action {
    pub const ALL: [Action; 18] = [
        Action::NewProject,
        Action::OpenProject,
        Action::Save,
        Action::SaveAs,
        Action::ExportTextures,
        Action::Undo,
        Action::Redo,
        Action::Delete,
        Action::FrameView,
        Action::BrushSmaller,
        Action::BrushBigger,
        Action::ToolPaint,
        Action::ToolEraser,
        Action::ToolProjection,
        Action::ToolPolygonFill,
        Action::ToolSmudge,
        Action::ToolClone,
        Action::ToolPath,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Action::NewProject => "New project",
            Action::OpenProject => "Open project",
            Action::Save => "Save",
            Action::SaveAs => "Save as",
            Action::ExportTextures => "Export textures",
            Action::Undo => "Undo",
            Action::Redo => "Redo",
            Action::Delete => "Delete layer",
            Action::FrameView => "Frame the model",
            Action::BrushSmaller => "Smaller brush",
            Action::BrushBigger => "Bigger brush",
            Action::ToolPaint => "Paint tool",
            Action::ToolEraser => "Eraser",
            Action::ToolProjection => "Projection tool",
            Action::ToolPolygonFill => "Polygon fill",
            Action::ToolSmudge => "Smudge",
            Action::ToolClone => "Clone",
            Action::ToolPath => "Path tool",
        }
    }
}

/// A key with exact modifiers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Shortcut {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub key: Key,
}

impl Shortcut {
    const fn new(key: Key) -> Self {
        Self {
            ctrl: false,
            shift: false,
            alt: false,
            key,
        }
    }

    const fn ctrl(key: Key) -> Self {
        Self {
            ctrl: true,
            ..Self::new(key)
        }
    }

    const fn ctrl_shift(key: Key) -> Self {
        Self {
            ctrl: true,
            shift: true,
            ..Self::new(key)
        }
    }

    /// "Ctrl+Shift+S"
    pub fn text(&self) -> String {
        let mut s = String::new();
        for (on, name) in [
            (self.ctrl, "Ctrl+"),
            (self.shift, "Shift+"),
            (self.alt, "Alt+"),
        ] {
            if on {
                s.push_str(name);
            }
        }
        s + self.key.name()
    }

    /// Like `text`, but with symbols for punctuation ("Ctrl+[" not
    /// "Ctrl+OpenBracket"). For display only: `parse` reads `text`.
    pub fn label(&self) -> String {
        let name = self.text();
        name[..name.len() - self.key.name().len()].to_owned() + self.key.symbol_or_name()
    }

    pub fn parse(text: &str) -> Option<Self> {
        let mut parts: Vec<&str> = text.split('+').collect();
        let key = Key::from_name(parts.pop()?)?;
        let mut s = Self::new(key);
        for p in parts {
            match p {
                "Ctrl" => s.ctrl = true,
                "Shift" => s.shift = true,
                "Alt" => s.alt = true,
                _ => return None,
            }
        }
        Some(s)
    }

    fn matches(&self, m: Modifiers) -> bool {
        m.command == self.ctrl && m.shift == self.shift && m.alt == self.alt
    }
}

/// How the 3D view is navigated with the mouse.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum Preset {
    /// Alt+LMB orbit, Alt+MMB / MMB pan, Alt+RMB zoom.
    #[default]
    Painter,
    /// MMB orbit, Shift+MMB pan, Ctrl+MMB zoom.
    Blender,
}

impl Preset {
    pub const ALL: [Preset; 2] = [Preset::Painter, Preset::Blender];

    pub fn label(self) -> &'static str {
        match self {
            Preset::Painter => "Substance Painter",
            Preset::Blender => "Blender",
        }
    }

    fn bindings(self) -> BTreeMap<Action, Vec<Shortcut>> {
        use Action as A;
        let mut b: BTreeMap<Action, Vec<Shortcut>> = BTreeMap::new();
        let tools = [
            (A::ToolPaint, Key::Num1),
            (A::ToolEraser, Key::Num2),
            (A::ToolProjection, Key::Num3),
            (A::ToolPolygonFill, Key::Num4),
            (A::ToolSmudge, Key::Num5),
            (A::ToolClone, Key::Num6),
            (A::ToolPath, Key::Num7),
        ];
        for (a, k) in tools {
            b.insert(a, vec![Shortcut::new(k)]);
        }
        b.insert(A::NewProject, vec![Shortcut::ctrl(Key::N)]);
        b.insert(A::OpenProject, vec![Shortcut::ctrl(Key::O)]);
        b.insert(A::Save, vec![Shortcut::ctrl(Key::S)]);
        b.insert(A::SaveAs, vec![Shortcut::ctrl_shift(Key::S)]);
        b.insert(A::ExportTextures, vec![Shortcut::ctrl_shift(Key::E)]);
        b.insert(A::Undo, vec![Shortcut::ctrl(Key::Z)]);
        b.insert(A::BrushSmaller, vec![Shortcut::new(Key::OpenBracket)]);
        b.insert(A::BrushBigger, vec![Shortcut::new(Key::CloseBracket)]);
        match self {
            Preset::Painter => {
                b.insert(
                    A::Redo,
                    vec![Shortcut::ctrl(Key::Y), Shortcut::ctrl_shift(Key::Z)],
                );
                b.insert(A::Delete, vec![Shortcut::new(Key::Delete)]);
                b.insert(A::FrameView, vec![Shortcut::new(Key::F)]);
            }
            Preset::Blender => {
                b.insert(A::Redo, vec![Shortcut::ctrl_shift(Key::Z)]);
                b.insert(
                    A::Delete,
                    vec![Shortcut::new(Key::X), Shortcut::new(Key::Delete)],
                );
                b.insert(
                    A::FrameView,
                    vec![Shortcut::new(Key::Home), Shortcut::new(Key::Period)],
                );
            }
        }
        b
    }
}

/// What Settings stores: the preset and the user's changes to it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeymapSettings {
    pub preset: Preset,
    /// Replaced bindings, as text ("Ctrl+Shift+S"); empty = unbound.
    pub overrides: BTreeMap<Action, Vec<String>>,
}

pub struct Keymap {
    pub preset: Preset,
    bindings: BTreeMap<Action, Vec<Shortcut>>,
}

impl Keymap {
    pub fn new(s: &KeymapSettings) -> Self {
        let mut bindings = s.preset.bindings();
        for (a, list) in &s.overrides {
            bindings.insert(*a, list.iter().filter_map(|t| Shortcut::parse(t)).collect());
        }
        Self {
            preset: s.preset,
            bindings,
        }
    }

    pub fn shortcuts(&self, a: Action) -> &[Shortcut] {
        self.bindings.get(&a).map_or(&[], |v| v.as_slice())
    }

    /// The first binding, for menus ("" when unbound).
    pub fn text(&self, a: Action) -> String {
        self.shortcuts(a)
            .first()
            .map(Shortcut::label)
            .unwrap_or_default()
    }

    /// Was `a` pressed this frame, without consuming it (several views react).
    pub fn down(&self, ctx: &egui::Context, a: Action) -> bool {
        ctx.input(|i| {
            i.events.iter().any(|e| match e {
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => self
                    .shortcuts(a)
                    .iter()
                    .any(|s| s.key == *key && s.matches(*modifiers)),
                _ => false,
            })
        })
    }

    /// Was `a` pressed this frame (modifiers exactly as bound)? Consumes the key.
    pub fn pressed(&self, ctx: &egui::Context, a: Action) -> bool {
        let list = self.shortcuts(a);
        if list.is_empty() {
            return false;
        }
        ctx.input_mut(|i| {
            let hit = i.events.iter().position(|e| match e {
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => list.iter().any(|s| s.key == *key && s.matches(*modifiers)),
                _ => false,
            });
            if let Some(k) = hit {
                i.events.remove(k);
            }
            hit.is_some()
        })
    }
}

/// The next key pressed with its modifiers (for rebinding), ignoring Escape.
pub fn captured(ctx: &egui::Context) -> Option<Shortcut> {
    ctx.input(|i| {
        i.events.iter().find_map(|e| match e {
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } if *key != Key::Escape => Some(Shortcut {
                ctrl: modifiers.command,
                shift: modifiers.shift,
                alt: modifiers.alt,
                key: *key,
            }),
            _ => None,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcuts_round_trip_and_overrides_apply() {
        let s = Shortcut::ctrl_shift(Key::S);
        assert_eq!(s.text(), "Ctrl+Shift+S");
        assert_eq!(Shortcut::parse("Ctrl+Shift+S"), Some(s));
        assert_eq!(Shortcut::parse("Hyper+S"), None);
        assert_eq!(Shortcut::ctrl(Key::OpenBracket).label(), "Ctrl+[");
        assert_eq!(Shortcut::new(Key::Period).label(), ".");
        let mut settings = KeymapSettings::default();
        let k = Keymap::new(&settings);
        assert_eq!(k.text(Action::FrameView), "F");
        settings.preset = Preset::Blender;
        settings
            .overrides
            .insert(Action::Save, vec!["Ctrl+W".into()]);
        let k = Keymap::new(&settings);
        assert_eq!(k.text(Action::FrameView), "Home");
        assert_eq!(k.text(Action::Save), "Ctrl+W");
    }

    #[test]
    fn modifiers_must_match_exactly() {
        let ctx = egui::Context::default();
        let k = Keymap::new(&KeymapSettings::default());
        let press = |key, modifiers| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        };
        let input = egui::RawInput {
            events: vec![press(Key::S, Modifiers::COMMAND | Modifiers::SHIFT)],
            ..Default::default()
        };
        let mut saved = (false, false);
        let mut out = ctx.run_ui(input, |ui| {
            saved = (
                k.pressed(ui.ctx(), Action::Save),
                k.pressed(ui.ctx(), Action::SaveAs),
            );
        });
        out.textures_delta.clear();
        assert_eq!(saved, (false, true), "Ctrl+Shift+S is Save as, not Save");
    }
}
