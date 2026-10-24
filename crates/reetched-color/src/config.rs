//! OCIO config model and processor construction.
//!
//! The way color space, display/view and look conversions are chained mirrors OpenColorIO's
//! BuildColorSpaceOps / BuildDisplayOps / BuildLookOps (Copyright Contributors to the
//! OpenColorIO Project, BSD-3-Clause), including the v2 split between the scene-referred and
//! the display-referred reference spaces.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow, bail, ensure};
use serde_yaml::Value;

use crate::builtins::builtin_ops;
use crate::lutfile;
use crate::ops::{EcStyle, GammaKind, LogParams, Op, range_scale_offset};
use crate::processor::Processor;

type Loader = Box<dyn Fn(&str) -> Result<Vec<u8>> + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferenceSpace {
    /// Scene-referred (`colorspaces`).
    Scene,
    /// Display-referred (`display_colorspaces`).
    Display,
}

#[derive(Clone, Debug)]
pub struct ColorSpace {
    pub name: String,
    pub aliases: Vec<String>,
    pub family: String,
    pub description: String,
    pub encoding: String,
    pub categories: Vec<String>,
    pub equality_group: String,
    pub is_data: bool,
    pub reference: ReferenceSpace,
    to_ref: Option<Value>,
    from_ref: Option<Value>,
}

impl ColorSpace {
    fn matches(&self, name: &str) -> bool {
        self.name.eq_ignore_ascii_case(name)
            || self.aliases.iter().any(|a| a.eq_ignore_ascii_case(name))
    }
}

struct NamedTransform {
    name: String,
    aliases: Vec<String>,
    fwd: Option<Value>,
    inv: Option<Value>,
}

#[derive(Clone)]
struct View {
    name: String,
    /// `colorspace`, or `display_colorspace` when there is a view transform.
    colorspace: String,
    view_transform: Option<String>,
    looks: String,
}

enum ViewEntry {
    Own(View),
    Shared(String),
}

struct Display {
    name: String,
    views: Vec<ViewEntry>,
}

struct ViewTransform {
    name: String,
    reference: ReferenceSpace,
    to_ref: Option<Value>,
    from_ref: Option<Value>,
}

struct Look {
    name: String,
    process_space: String,
    fwd: Option<Value>,
    inv: Option<Value>,
}

/// A loaded OCIO config (v1 or v2 YAML).
pub struct Config {
    pub name: String,
    pub description: String,
    roles: Vec<(String, String)>,
    spaces: Vec<ColorSpace>,
    named: Vec<NamedTransform>,
    displays: Vec<Display>,
    shared_views: Vec<View>,
    view_transforms: Vec<ViewTransform>,
    default_view_transform: Option<String>,
    looks: Vec<Look>,
    active_displays: Vec<String>,
    active_views: Vec<String>,
    inactive: Vec<String>,
    use_display_aliases: bool,
    search_paths: Vec<String>,
    env: Vec<(String, String)>,
    loader: Loader,
    files: Mutex<HashMap<String, Arc<Vec<Op>>>>,
}

// ---- YAML helpers -------------------------------------------------------------------------

/// Rewrites OCIO's verbatim tags `!<ColorSpace>` as local tags `!ColorSpace`: serde_yaml
/// drops verbatim tags but keeps local ones.
fn local_tags(yaml: &str) -> String {
    let mut out = String::with_capacity(yaml.len());
    let mut rest = yaml;
    while let Some(i) = rest.find("!<") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        match after.find('>') {
            Some(end)
                if end > 0
                    && after[..end]
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_') =>
            {
                out.push('!');
                out.push_str(&after[..end]);
                rest = &after[end + 1..];
            }
            _ => {
                out.push_str("!<");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Splits an OCIO `!<Tag>` node into its tag name ("" when untagged) and body.
fn untag(v: &Value) -> (String, &Value) {
    match v {
        Value::Tagged(t) => {
            let tag = t.tag.to_string();
            (
                tag.trim_start_matches('!')
                    .trim_start_matches('<')
                    .trim_end_matches('>')
                    .to_string(),
                &t.value,
            )
        }
        other => (String::new(), other),
    }
}

fn get<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    untag(v).1.as_mapping()?.get(key).filter(|v| !v.is_null())
}

fn scalar(v: &Value) -> Option<String> {
    match untag(v).1 {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn get_str(v: &Value, key: &str) -> Option<String> {
    get(v, key).and_then(scalar)
}

fn get_list(v: &Value, key: &str) -> Vec<String> {
    match get(v, key).map(|v| untag(v).1) {
        Some(Value::Sequence(s)) => s.iter().filter_map(scalar).collect(),
        Some(other) => scalar(other)
            .map(|s| {
                s.split(',')
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
        None => vec![],
    }
}

fn get_f(v: &Value, key: &str) -> Result<Option<f64>> {
    get(v, key)
        .map(|x| {
            untag(x)
                .1
                .as_f64()
                .ok_or_else(|| anyhow!("'{key}' must be a number"))
        })
        .transpose()
}

fn get_fs(v: &Value, key: &str) -> Result<Option<Vec<f64>>> {
    match get(v, key).map(|x| untag(x).1) {
        None => Ok(None),
        Some(Value::Sequence(s)) => Ok(Some(
            s.iter()
                .map(|x| x.as_f64().ok_or_else(|| anyhow!("'{key}' must be numbers")))
                .collect::<Result<_>>()?,
        )),
        Some(x) => Ok(Some(vec![
            x.as_f64()
                .ok_or_else(|| anyhow!("'{key}' must be a number"))?,
        ])),
    }
}

/// Scalar or per-channel (3 or 4 values) parameter.
fn get_rgb(v: &Value, key: &str, default: f64) -> Result<[f64; 3]> {
    Ok(match get_fs(v, key)? {
        None => [default; 3],
        Some(x) if x.len() == 1 => [x[0]; 3],
        Some(x) if x.len() >= 3 => [x[0], x[1], x[2]],
        Some(_) => bail!("'{key}' needs 1, 3 or 4 values"),
    })
}

fn get_bool(v: &Value, key: &str) -> bool {
    get(v, key)
        .and_then(|v| untag(v).1.as_bool())
        .unwrap_or(false)
}

fn eq(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

// ---- loading ------------------------------------------------------------------------------

impl Config {
    /// Parses a config from YAML text. `loader` receives LUT paths (already joined with the
    /// config's search path) and returns the file bytes.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(
        yaml: &str,
        loader: impl Fn(&str) -> Result<Vec<u8>> + Send + Sync + 'static,
    ) -> Result<Config> {
        let root: Value =
            serde_yaml::from_str(&local_tags(yaml)).context("parsing OCIO config YAML")?;
        ensure!(root.is_mapping(), "OCIO config must be a YAML mapping");

        let roles = match get(&root, "roles").and_then(|r| r.as_mapping()) {
            Some(m) => m
                .iter()
                .filter_map(|(k, v)| Some((scalar(k)?, scalar(v)?)))
                .collect(),
            None => vec![],
        };

        let mut spaces = Vec::new();
        for (key, reference) in [
            ("display_colorspaces", ReferenceSpace::Display),
            ("colorspaces", ReferenceSpace::Scene),
        ] {
            for cs in get(&root, key)
                .and_then(|v| v.as_sequence())
                .into_iter()
                .flatten()
            {
                spaces.push(parse_colorspace(cs, reference)?);
            }
        }

        let named = get(&root, "named_transforms")
            .and_then(|v| v.as_sequence())
            .into_iter()
            .flatten()
            .map(|nt| {
                Ok(NamedTransform {
                    name: get_str(nt, "name")
                        .ok_or_else(|| anyhow!("named transform without name"))?,
                    aliases: get_list(nt, "aliases"),
                    fwd: get(nt, "transform").cloned(),
                    inv: get(nt, "inverse_transform").cloned(),
                })
            })
            .collect::<Result<_>>()?;

        let mut displays = Vec::new();
        if let Some(m) = get(&root, "displays").and_then(|v| v.as_mapping()) {
            for (name, views) in m {
                let name = scalar(name).ok_or_else(|| anyhow!("bad display name"))?;
                let mut entries = Vec::new();
                for v in views.as_sequence().into_iter().flatten() {
                    let (tag, body) = untag(v);
                    if tag == "Views" {
                        entries.extend(
                            body.as_sequence()
                                .into_iter()
                                .flatten()
                                .filter_map(scalar)
                                .map(ViewEntry::Shared),
                        );
                    } else {
                        entries.push(ViewEntry::Own(parse_view(v)?));
                    }
                }
                displays.push(Display {
                    name,
                    views: entries,
                });
            }
        }

        let shared_views = get(&root, "shared_views")
            .and_then(|v| v.as_sequence())
            .into_iter()
            .flatten()
            .map(parse_view)
            .collect::<Result<_>>()?;

        let view_transforms = get(&root, "view_transforms")
            .and_then(|v| v.as_sequence())
            .into_iter()
            .flatten()
            .map(|vt| {
                let name =
                    get_str(vt, "name").ok_or_else(|| anyhow!("view transform without name"))?;
                let (scene_to, scene_from) = (
                    get(vt, "to_scene_reference"),
                    get(vt, "from_scene_reference"),
                );
                let (disp_to, disp_from) = (
                    get(vt, "to_display_reference"),
                    get(vt, "from_display_reference"),
                );
                let scene = scene_to.is_some() || scene_from.is_some();
                Ok(ViewTransform {
                    name,
                    reference: if scene {
                        ReferenceSpace::Scene
                    } else {
                        ReferenceSpace::Display
                    },
                    to_ref: if scene { scene_to } else { disp_to }.cloned(),
                    from_ref: if scene { scene_from } else { disp_from }.cloned(),
                })
            })
            .collect::<Result<_>>()?;

        let looks = get(&root, "looks")
            .and_then(|v| v.as_sequence())
            .into_iter()
            .flatten()
            .map(|l| {
                Ok(Look {
                    name: get_str(l, "name").ok_or_else(|| anyhow!("look without name"))?,
                    process_space: get_str(l, "process_space").unwrap_or_default(),
                    fwd: get(l, "transform").cloned(),
                    inv: get(l, "inverse_transform").cloned(),
                })
            })
            .collect::<Result<_>>()?;

        let search_paths = match get(&root, "search_path").map(|v| untag(v).1) {
            Some(Value::Sequence(s)) => s.iter().filter_map(scalar).collect(),
            Some(v) => scalar(v)
                .map(|s| s.split(':').map(str::to_string).collect())
                .unwrap_or_default(),
            None => vec![],
        };
        let env = match get(&root, "environment").and_then(|v| v.as_mapping()) {
            Some(m) => m
                .iter()
                .filter_map(|(k, v)| Some((scalar(k)?, scalar(v).unwrap_or_default())))
                .collect(),
            None => vec![],
        };

        Ok(Config {
            name: get_str(&root, "name").unwrap_or_default(),
            description: get_str(&root, "description").unwrap_or_default(),
            roles,
            spaces,
            named,
            displays,
            shared_views,
            view_transforms,
            default_view_transform: get_str(&root, "default_view_transform"),
            looks,
            active_displays: get_list(&root, "active_displays"),
            active_views: get_list(&root, "active_views"),
            inactive: get_list(&root, "inactive_colorspaces"),
            use_display_aliases: get_bool(&root, "use_display_aliases"),
            search_paths,
            env,
            loader: Box::new(loader),
            files: Mutex::new(HashMap::new()),
        })
    }

    /// Loads a config file; LUTs are resolved relative to the config's directory.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Config> {
        let path = path.as_ref();
        let yaml =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        Config::from_str(&yaml, move |p| {
            let full = dir.join(p);
            std::fs::read(&full).with_context(|| format!("reading {}", full.display()))
        })
    }

    /// One of the configs from [`crate::builtin_configs`], by name.
    pub fn builtin(name: &str) -> Result<Config> {
        let (_, yaml) = crate::builtin_configs()
            .iter()
            .find(|(n, _)| *n == name)
            .ok_or_else(|| anyhow!("no built-in config named '{name}'"))?;
        Config::from_str(yaml, |p| {
            bail!("built-in configs reference no files (asked for '{p}')")
        })
    }
}

fn parse_colorspace(cs: &Value, reference: ReferenceSpace) -> Result<ColorSpace> {
    let name = get_str(cs, "name").ok_or_else(|| anyhow!("color space without name"))?;
    let (to, from) = match reference {
        ReferenceSpace::Scene => ("to_scene_reference", "from_scene_reference"),
        ReferenceSpace::Display => ("to_display_reference", "from_display_reference"),
    };
    Ok(ColorSpace {
        aliases: get_list(cs, "aliases"),
        family: get_str(cs, "family").unwrap_or_default(),
        description: get_str(cs, "description").unwrap_or_default(),
        encoding: get_str(cs, "encoding").unwrap_or_default(),
        categories: get_list(cs, "categories"),
        equality_group: get_str(cs, "equalitygroup").unwrap_or_default(),
        is_data: get_bool(cs, "isdata"),
        reference,
        to_ref: get(cs, to).or_else(|| get(cs, "to_reference")).cloned(),
        from_ref: get(cs, from).or_else(|| get(cs, "from_reference")).cloned(),
        name,
    })
}

fn parse_view(v: &Value) -> Result<View> {
    Ok(View {
        name: get_str(v, "name").ok_or_else(|| anyhow!("view without name"))?,
        colorspace: get_str(v, "display_colorspace")
            .or_else(|| get_str(v, "colorspace"))
            .unwrap_or_default(),
        view_transform: get_str(v, "view_transform"),
        looks: get_str(v, "looks").unwrap_or_default(),
    })
}

// ---- queries ------------------------------------------------------------------------------

impl Config {
    /// All color spaces (scene- and display-referred, active or not) in config order.
    pub fn color_spaces(&self) -> &[ColorSpace] {
        &self.spaces
    }

    /// Names of the active color spaces (not listed in `inactive_colorspaces`).
    pub fn color_space_names(&self) -> Vec<&str> {
        self.spaces
            .iter()
            .filter(|cs| !self.inactive.iter().any(|i| eq(i, &cs.name)))
            .map(|cs| cs.name.as_str())
            .collect()
    }

    /// Looks up a color space by name, alias or role (case-insensitive like OCIO).
    pub fn color_space(&self, name: &str) -> Option<&ColorSpace> {
        self.spaces.iter().find(|cs| cs.matches(name)).or_else(|| {
            let target = self.role(name)?;
            self.spaces.iter().find(|cs| cs.matches(target))
        })
    }

    pub fn roles(&self) -> &[(String, String)] {
        &self.roles
    }

    pub fn role(&self, role: &str) -> Option<&str> {
        self.roles
            .iter()
            .find(|(r, _)| eq(r, role))
            .map(|(_, cs)| cs.as_str())
    }

    pub fn looks(&self) -> Vec<&str> {
        self.looks.iter().map(|l| l.name.as_str()).collect()
    }

    pub fn named_transforms(&self) -> Vec<&str> {
        self.named.iter().map(|n| n.name.as_str()).collect()
    }

    /// Active displays, ordered by `active_displays` when present.
    pub fn displays(&self) -> Vec<&str> {
        let all: Vec<&str> = self.displays.iter().map(|d| d.name.as_str()).collect();
        let active: Vec<&str> = self
            .active_displays
            .iter()
            .filter_map(|a| all.iter().find(|d| eq(d, a)).copied())
            .collect();
        if active.is_empty() { all } else { active }
    }

    pub fn default_display(&self) -> Option<&str> {
        self.displays().first().copied()
    }

    /// Active views of a display, ordered by `active_views` when present.
    pub fn views(&self, display: &str) -> Vec<&str> {
        let Some(d) = self.find_display(display) else {
            return vec![];
        };
        let all: Vec<&str> = d
            .views
            .iter()
            .map(|v| match v {
                ViewEntry::Own(v) => v.name.as_str(),
                ViewEntry::Shared(n) => n.as_str(),
            })
            .collect();
        let active: Vec<&str> = self
            .active_views
            .iter()
            .filter_map(|a| all.iter().find(|v| eq(v, a)).copied())
            .collect();
        if active.is_empty() { all } else { active }
    }

    pub fn default_view(&self, display: &str) -> Option<&str> {
        self.views(display).first().copied()
    }

    fn find_display(&self, name: &str) -> Option<&Display> {
        if let Some(d) = self.displays.iter().find(|d| eq(&d.name, name)) {
            return Some(d);
        }
        // Configs may let display color space names/aliases stand in for display names.
        let cs = self
            .color_space(name)
            .filter(|_| self.use_display_aliases)?;
        self.displays.iter().find(|d| eq(&d.name, &cs.name))
    }

    fn find_view(&self, display: &Display, view: &str) -> Result<View> {
        for entry in &display.views {
            match entry {
                ViewEntry::Own(v) if eq(&v.name, view) => return Ok(v.clone()),
                ViewEntry::Shared(n) if eq(n, view) => {
                    let mut v = self
                        .shared_views
                        .iter()
                        .find(|s| eq(&s.name, n))
                        .cloned()
                        .ok_or_else(|| anyhow!("shared view '{n}' is not defined"))?;
                    if v.colorspace == "<USE_DISPLAY_NAME>" {
                        v.colorspace = display.name.clone();
                    }
                    return Ok(v);
                }
                _ => {}
            }
        }
        bail!("display '{}' has no view '{view}'", display.name)
    }

    fn named_transform(&self, name: &str) -> Option<&NamedTransform> {
        self.named
            .iter()
            .find(|n| eq(&n.name, name) || n.aliases.iter().any(|a| eq(a, name)))
    }

    fn cs(&self, name: &str) -> Result<&ColorSpace> {
        self.color_space(name)
            .ok_or_else(|| anyhow!("color space '{name}' not found"))
    }
}

// ---- processors ---------------------------------------------------------------------------

impl Config {
    /// Color space (or named transform) conversion.
    pub fn processor(&self, src: &str, dst: &str) -> Result<Processor> {
        let mut ops = Vec::new();
        self.cs_names_ops(src, dst, true, &mut ops)
            .with_context(|| format!("building processor '{src}' -> '{dst}'"))?;
        Ok(Processor::new(ops))
    }

    /// Display/view conversion from `src`. `looks` overrides the view's own looks
    /// (`Some("")` disables them), like OCIO's LegacyViewingPipeline looks override.
    pub fn display_processor(
        &self,
        src: &str,
        display: &str,
        view: &str,
        looks: Option<&str>,
    ) -> Result<Processor> {
        let mut ops = Vec::new();
        self.display_ops(src, display, view, looks, false, &mut ops)
            .with_context(|| {
                format!("building display processor '{src}' -> '{display}' / '{view}'")
            })?;
        Ok(Processor::new(ops))
    }

    fn cs_names_ops(&self, src: &str, dst: &str, bypass: bool, ops: &mut Vec<Op>) -> Result<()> {
        match (self.color_space(src), self.color_space(dst)) {
            (Some(s), Some(d)) => self.cs_ops(s, d, bypass, ops),
            (s, d) => {
                // A named transform on either side replaces the reference-space round trip.
                let nt_s = if s.is_none() {
                    Some(
                        self.named_transform(src)
                            .ok_or_else(|| anyhow!("'{src}' not found"))?,
                    )
                } else {
                    None
                };
                let nt_d = if d.is_none() {
                    Some(
                        self.named_transform(dst)
                            .ok_or_else(|| anyhow!("'{dst}' not found"))?,
                    )
                } else {
                    None
                };
                if let Some(nt) = nt_s {
                    self.named_ops(nt, false, ops)?;
                }
                if let Some(nt) = nt_d {
                    self.named_ops(nt, true, ops)?;
                }
                Ok(())
            }
        }
    }

    fn named_ops(&self, nt: &NamedTransform, inverse: bool, ops: &mut Vec<Op>) -> Result<()> {
        let (primary, secondary) = if inverse {
            (&nt.inv, &nt.fwd)
        } else {
            (&nt.fwd, &nt.inv)
        };
        match (primary, secondary) {
            (Some(t), _) => self.transform_ops(t, false, ops),
            (None, Some(t)) => self.transform_ops(t, true, ops),
            _ => Ok(()),
        }
    }

    fn cs_ops(
        &self,
        s: &ColorSpace,
        d: &ColorSpace,
        bypass: bool,
        ops: &mut Vec<Op>,
    ) -> Result<()> {
        if eq(&s.name, &d.name)
            || (!s.equality_group.is_empty() && s.equality_group == d.equality_group)
        {
            return Ok(());
        }
        if bypass && (s.is_data || d.is_data) {
            return Ok(());
        }
        self.cs_to_ref_ops(s, ops)?;
        self.ref_conversion(s.reference, d.reference, ops)?;
        self.ref_to_cs_ops(d, ops)
    }

    fn cs_to_ref_ops(&self, cs: &ColorSpace, ops: &mut Vec<Op>) -> Result<()> {
        match (&cs.to_ref, &cs.from_ref) {
            (Some(t), _) => self.transform_ops(t, false, ops),
            (None, Some(t)) => self.transform_ops(t, true, ops),
            _ => Ok(()),
        }
    }

    fn ref_to_cs_ops(&self, cs: &ColorSpace, ops: &mut Vec<Op>) -> Result<()> {
        match (&cs.from_ref, &cs.to_ref) {
            (Some(t), _) => self.transform_ops(t, false, ops),
            (None, Some(t)) => self.transform_ops(t, true, ops),
            _ => Ok(()),
        }
    }

    fn vt_from_ref(&self, vt: &ViewTransform, ops: &mut Vec<Op>) -> Result<()> {
        match (&vt.from_ref, &vt.to_ref) {
            (Some(t), _) => self.transform_ops(t, false, ops),
            (None, Some(t)) => self.transform_ops(t, true, ops),
            _ => bail!("view transform '{}' has no transform", vt.name),
        }
    }

    fn vt_to_ref(&self, vt: &ViewTransform, ops: &mut Vec<Op>) -> Result<()> {
        match (&vt.to_ref, &vt.from_ref) {
            (Some(t), _) => self.transform_ops(t, false, ops),
            (None, Some(t)) => self.transform_ops(t, true, ops),
            _ => bail!("view transform '{}' has no transform", vt.name),
        }
    }

    /// Converts between the scene and display reference spaces with the default view transform.
    fn ref_conversion(
        &self,
        src: ReferenceSpace,
        dst: ReferenceSpace,
        ops: &mut Vec<Op>,
    ) -> Result<()> {
        if src == dst {
            return Ok(());
        }
        let scene_vts = || {
            self.view_transforms
                .iter()
                .filter(|v| v.reference == ReferenceSpace::Scene)
        };
        let vt = self
            .default_view_transform
            .as_deref()
            .and_then(|n| scene_vts().find(|v| eq(&v.name, n)))
            .or_else(|| scene_vts().next())
            .ok_or_else(|| {
                anyhow!("no scene-referred view transform to reach the display reference space")
            })?;
        if src == ReferenceSpace::Scene {
            self.vt_from_ref(vt, ops)
        } else {
            self.vt_to_ref(vt, ops)
        }
    }

    fn display_ops(
        &self,
        src: &str,
        display: &str,
        view: &str,
        looks: Option<&str>,
        inverse: bool,
        ops: &mut Vec<Op>,
    ) -> Result<()> {
        let src_cs = self.cs(src)?;
        let d = self
            .find_display(display)
            .ok_or_else(|| anyhow!("display '{display}' not found"))?;
        let v = self.find_view(d, view)?;
        let disp_cs = self.color_space(&v.colorspace);
        if src_cs.is_data || disp_cs.is_some_and(|c| c.is_data) {
            // OCIO's looks override still applies here, straight to the input values.
            if let Some(l) = looks.filter(|l| !l.trim().is_empty() && !inverse) {
                self.look_ops(src_cs, l, false, false, ops)?;
            }
            return Ok(());
        }
        let looks = looks.unwrap_or(&v.looks);
        let vt = v.view_transform.as_deref();
        let vt_obj = vt.and_then(|n| self.view_transforms.iter().find(|t| eq(&t.name, n)));
        let vt_named = match (vt, vt_obj) {
            (Some(n), None) => Some(
                self.named_transform(n)
                    .ok_or_else(|| anyhow!("view transform '{n}' not found"))?,
            ),
            _ => None,
        };
        let disp =
            || disp_cs.ok_or_else(|| anyhow!("display color space '{}' not found", v.colorspace));

        if !inverse {
            let mut cur = src_cs;
            if !looks.trim().is_empty() {
                cur = self.look_ops(cur, looks, false, true, ops)?;
            }
            if let Some(vt) = vt_obj {
                self.cs_to_ref_ops(cur, ops)?;
                self.ref_conversion(cur.reference, vt.reference, ops)?;
                self.vt_from_ref(vt, ops)?;
                self.ref_to_cs_ops(disp()?, ops)?;
            } else if let Some(nt) = vt_named {
                self.named_ops(nt, false, ops)?;
                self.ref_to_cs_ops(disp()?, ops)?;
            } else if let Some(dcs) = disp_cs {
                self.cs_ops(cur, dcs, true, ops)?;
            } else {
                let nt = self
                    .named_transform(&v.colorspace)
                    .ok_or_else(|| anyhow!("'{}' not found", v.colorspace))?;
                self.named_ops(nt, false, ops)?;
            }
        } else {
            let vt_src = if looks.trim().is_empty() {
                src_cs
            } else {
                self.look_ops(src_cs, looks, false, true, &mut Vec::new())?
            };
            if let Some(vt) = vt_obj {
                self.cs_to_ref_ops(disp()?, ops)?;
                self.vt_to_ref(vt, ops)?;
                self.ref_conversion(vt.reference, vt_src.reference, ops)?;
                self.ref_to_cs_ops(vt_src, ops)?;
            } else if let Some(nt) = vt_named {
                self.cs_to_ref_ops(disp()?, ops)?;
                self.named_ops(nt, true, ops)?;
            } else if let Some(dcs) = disp_cs {
                self.cs_ops(dcs, vt_src, true, ops)?;
            } else {
                let nt = self
                    .named_transform(&v.colorspace)
                    .ok_or_else(|| anyhow!("'{}' not found", v.colorspace))?;
                self.named_ops(nt, true, ops)?;
            }
            if !looks.trim().is_empty() {
                let end = self.look_ops(vt_src, looks, true, true, ops)?;
                self.cs_ops(end, src_cs, true, ops)?;
            }
        }
        Ok(())
    }

    /// Applies a look string ("a, -b | c": `|` separates fallbacks) starting from `cur`;
    /// returns the color space the result is in. `convert: false` skips the conversions to
    /// the looks' process spaces.
    fn look_ops<'a>(
        &'a self,
        cur: &'a ColorSpace,
        looks: &str,
        reverse: bool,
        convert: bool,
        ops: &mut Vec<Op>,
    ) -> Result<&'a ColorSpace> {
        let mut last_err = None;
        for option in looks.split('|') {
            let sep = if option.contains(',') { ',' } else { ':' };
            let mut tokens: Vec<(&str, bool)> = option
                .split(sep)
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(|t| match t.strip_prefix('-') {
                    Some(n) => (n, true),
                    None => (t.trim_start_matches('+'), false),
                })
                .collect();
            if reverse {
                tokens.reverse();
                tokens.iter_mut().for_each(|t| t.1 = !t.1);
            }
            let mut tmp = Vec::new();
            match self.run_looks(cur, &tokens, convert, &mut tmp) {
                Ok(cs) => {
                    ops.extend(tmp);
                    return Ok(cs);
                }
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow!("empty look string")))
    }

    fn run_looks<'a>(
        &'a self,
        mut cur: &'a ColorSpace,
        tokens: &[(&str, bool)],
        convert: bool,
        ops: &mut Vec<Op>,
    ) -> Result<&'a ColorSpace> {
        for &(name, inverse) in tokens {
            let look = self
                .looks
                .iter()
                .find(|l| eq(&l.name, name))
                .ok_or_else(|| anyhow!("look '{name}' not found"))?;
            let mut tmp = Vec::new();
            let (primary, secondary) = if inverse {
                (&look.inv, &look.fwd)
            } else {
                (&look.fwd, &look.inv)
            };
            match (primary, secondary) {
                (Some(t), _) => self.transform_ops(t, false, &mut tmp)?,
                (None, Some(t)) => self.transform_ops(t, true, &mut tmp)?,
                _ => {}
            }
            let process = self.cs(&look.process_space)?;
            if convert && !eq(&cur.name, &process.name) {
                self.cs_ops(cur, process, true, ops)?;
                cur = process;
            }
            ops.extend(tmp);
        }
        Ok(cur)
    }

    fn expand(&self, s: &str) -> String {
        let mut out = s.to_string();
        for (k, v) in &self.env {
            let val = std::env::var(k).unwrap_or_else(|_| v.clone());
            out = out
                .replace(&format!("${{{k}}}"), &val)
                .replace(&format!("${k}"), &val);
        }
        out
    }

    fn file_ops(&self, src: &str) -> Result<Arc<Vec<Op>>> {
        let src = self.expand(src);
        if let Some(ops) = self.files.lock().unwrap().get(&src) {
            return Ok(ops.clone());
        }
        let mut candidates: Vec<String> = Vec::new();
        for sp in &self.search_paths {
            let sp = self.expand(sp);
            if sp.is_empty() || sp == "." {
                candidates.push(src.clone());
            } else {
                candidates.push(format!("{}/{}", sp.trim_end_matches(['/', '\\']), src));
            }
        }
        candidates.push(src.clone());
        let mut last_err = anyhow!("file '{src}' not found");
        for path in candidates {
            match (self.loader)(&path) {
                Ok(bytes) => {
                    let ops = Arc::new(lutfile::load(&path, &bytes)?);
                    self.files.lock().unwrap().insert(src, ops.clone());
                    return Ok(ops);
                }
                Err(e) => last_err = e,
            }
        }
        Err(last_err)
    }

    /// Appends the ops of a YAML transform; `inverse` combines with the transform's own direction.
    fn transform_ops(&self, t: &Value, inverse: bool, ops: &mut Vec<Op>) -> Result<()> {
        let (tag, _) = untag(t);
        let inverse = inverse ^ (get_str(t, "direction").as_deref() == Some("inverse"));
        match tag.as_str() {
            "GroupTransform" => {
                let children: Vec<&Value> = get(t, "children")
                    .and_then(|c| c.as_sequence())
                    .into_iter()
                    .flatten()
                    .collect();
                if inverse {
                    children
                        .iter()
                        .rev()
                        .try_for_each(|c| self.transform_ops(c, true, ops))
                } else {
                    children
                        .iter()
                        .try_for_each(|c| self.transform_ops(c, false, ops))
                }
            }
            "ColorSpaceTransform" => {
                let (mut src, mut dst) = (
                    get_str(t, "src").unwrap_or_default(),
                    get_str(t, "dst").unwrap_or_default(),
                );
                if inverse {
                    std::mem::swap(&mut src, &mut dst);
                }
                let bypass = get(t, "data_bypass")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                self.cs_names_ops(&src, &dst, bypass, ops)
            }
            "DisplayViewTransform" => {
                let looks = if get_bool(t, "looks_bypass") {
                    Some("")
                } else {
                    None
                };
                self.display_ops(
                    &get_str(t, "src").unwrap_or_default(),
                    &get_str(t, "display").unwrap_or_default(),
                    &get_str(t, "view").unwrap_or_default(),
                    looks,
                    inverse,
                    ops,
                )
            }
            "LookTransform" => {
                let mut src = self.cs(&get_str(t, "src").unwrap_or_default())?;
                let mut dst = self.cs(&get_str(t, "dst").unwrap_or_default())?;
                if inverse {
                    std::mem::swap(&mut src, &mut dst);
                }
                let looks = get_str(t, "looks").unwrap_or_default();
                let cur = if looks.trim().is_empty() {
                    src
                } else {
                    self.look_ops(src, &looks, inverse, true, ops)?
                };
                self.cs_ops(cur, dst, true, ops)
            }
            _ => {
                let mut tmp = Vec::new();
                self.simple_ops(&tag, t, &mut tmp)
                    .with_context(|| format!("in !<{tag}>"))?;
                if inverse {
                    ops.extend(tmp.iter().rev().map(Op::inverse));
                } else {
                    ops.extend(tmp);
                }
                Ok(())
            }
        }
    }

    /// Forward ops of a self-contained transform.
    fn simple_ops(&self, tag: &str, t: &Value, ops: &mut Vec<Op>) -> Result<()> {
        match tag {
            "MatrixTransform" => {
                let m = get_fs(t, "matrix")?.unwrap_or_else(|| {
                    vec![
                        1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
                    ]
                });
                let off = get_fs(t, "offset")?.unwrap_or_else(|| vec![0.; 4]);
                ensure!(
                    m.len() == 16 && off.len() == 4,
                    "matrix needs 16 values and offset 4"
                );
                // ponytail: the alpha row/column of the 4x4 matrix is ignored (RGB only).
                ops.push(Op::Matrix {
                    m: [[m[0], m[1], m[2]], [m[4], m[5], m[6]], [m[8], m[9], m[10]]],
                    off: [off[0], off[1], off[2]],
                });
            }
            "ExponentTransform" => {
                let kind = match get_str(t, "style").as_deref() {
                    Some("pass_thru") => GammaKind::PassThru,
                    Some("mirror") => GammaKind::Mirror,
                    _ => GammaKind::Basic,
                };
                ops.push(Op::Gamma {
                    kind,
                    inverse: false,
                    gamma: get_rgb(t, "value", 1.)?,
                    offset: [0.; 3],
                });
            }
            "ExponentWithLinearTransform" => {
                let kind = if get_str(t, "style").as_deref() == Some("mirror") {
                    GammaKind::MoncurveMirror
                } else {
                    GammaKind::Moncurve
                };
                ops.push(Op::Gamma {
                    kind,
                    inverse: false,
                    gamma: get_rgb(t, "gamma", 1.)?,
                    offset: get_rgb(t, "offset", 0.)?,
                });
            }
            "LogTransform" => ops.push(Op::Log {
                base: get_f(t, "base")?.unwrap_or(2.),
                p: [LogParams::PLAIN; 3],
                inverse: false,
            }),
            "LogAffineTransform" | "LogCameraTransform" => {
                let camera = tag == "LogCameraTransform";
                let (ls, lo) = (
                    get_rgb(t, "log_side_slope", 1.)?,
                    get_rgb(t, "log_side_offset", 0.)?,
                );
                let (ms, mo) = (
                    get_rgb(t, "lin_side_slope", 1.)?,
                    get_rgb(t, "lin_side_offset", 0.)?,
                );
                let brk = get_fs(t, "lin_side_break")?;
                ensure!(
                    !camera || brk.is_some(),
                    "LogCameraTransform needs lin_side_break"
                );
                let brk = get_rgb(t, "lin_side_break", 0.)?;
                let lin_slope = get_fs(t, "linear_slope")?
                    .map(|_| get_rgb(t, "linear_slope", 1.))
                    .transpose()?;
                let p = [0, 1, 2].map(|c| LogParams {
                    log_slope: ls[c],
                    log_offset: lo[c],
                    lin_slope: ms[c],
                    lin_offset: mo[c],
                    lin_break: camera.then_some(brk[c]),
                    linear_slope: lin_slope.map(|s| s[c]),
                });
                ops.push(Op::Log {
                    base: get_f(t, "base")?.unwrap_or(2.),
                    p,
                    inverse: false,
                });
            }
            "RangeTransform" => {
                let (min_in, max_in) = (get_f(t, "min_in_value")?, get_f(t, "max_in_value")?);
                let (min_out, max_out) = (get_f(t, "min_out_value")?, get_f(t, "max_out_value")?);
                let no_clamp =
                    get_str(t, "style").is_some_and(|s| s.eq_ignore_ascii_case("noClamp"));
                if no_clamp {
                    let (s, o) = range_scale_offset(min_in, max_in, min_out, max_out);
                    ops.push(Op::scale_offset([s; 3], [o; 3]));
                } else {
                    ops.push(Op::Range {
                        min_in,
                        max_in,
                        min_out,
                        max_out,
                    });
                }
            }
            "CDLTransform" => {
                // OCIO v2 defaults CDLTransform to noClamp; only "asc" clamps.
                let style = get_str(t, "style").unwrap_or_default();
                ops.push(Op::Cdl {
                    slope: get_rgb(t, "slope", 1.)?,
                    offset: get_rgb(t, "offset", 0.)?,
                    power: get_rgb(t, "power", 1.)?,
                    sat: get_f(t, "sat")?.unwrap_or(1.),
                    clamp: style.eq_ignore_ascii_case("asc"),
                    inverse: false,
                });
            }
            "ExposureContrastTransform" => {
                let style = match get_str(t, "style")
                    .unwrap_or_default()
                    .to_ascii_lowercase()
                    .as_str()
                {
                    "video" => EcStyle::Video,
                    "log" | "logarithmic" => EcStyle::Log,
                    _ => EcStyle::Linear,
                };
                ops.push(Op::ExposureContrast {
                    style,
                    exposure: get_f(t, "exposure")?.unwrap_or(0.),
                    contrast: get_f(t, "contrast")?.unwrap_or(1.),
                    gamma: get_f(t, "gamma")?.unwrap_or(1.),
                    pivot: get_f(t, "pivot")?.unwrap_or(0.18),
                    log_step: get_f(t, "log_exposure_step")?.unwrap_or(0.088),
                    log_mid: get_f(t, "log_midway_gray")?.unwrap_or(0.435),
                    inverse: false,
                });
            }
            "FixedFunctionTransform" => {
                let style = get_str(t, "style").unwrap_or_default().to_ascii_lowercase();
                let p = get_fs(t, "params")?.unwrap_or_default();
                let need = |n: usize| -> Result<()> {
                    ensure!(p.len() == n, "FixedFunction '{style}' needs {n} params");
                    Ok(())
                };
                ops.push(match style.as_str() {
                    "aces_redmod03" => Op::RedMod {
                        v03: true,
                        inverse: false,
                    },
                    "aces_redmod10" => Op::RedMod {
                        v03: false,
                        inverse: false,
                    },
                    "aces_glow03" => Op::Glow {
                        gain: 0.075,
                        mid: 0.1,
                        inverse: false,
                    },
                    "aces_glow10" => Op::Glow {
                        gain: 0.05,
                        mid: 0.08,
                        inverse: false,
                    },
                    "aces_darktodim10" => Op::DarkToDim { gamma: 0.9811 },
                    "aces_gamutcomp13" => {
                        need(7)?;
                        let f = |i: usize| p[i] as f32;
                        Op::GamutComp13 {
                            lim: [f(0), f(1), f(2)],
                            thr: [f(3), f(4), f(5)],
                            power: f(6),
                            inverse: false,
                        }
                    }
                    "aces2_outputtransform" => {
                        need(9)?;
                        let lim = crate::builtins::Primaries {
                            r: [p[1], p[2]],
                            g: [p[3], p[4]],
                            b: [p[5], p[6]],
                            w: [p[7], p[8]],
                        };
                        Op::Aces2 {
                            t: Arc::new(crate::aces2::Aces2::new(p[0] as f32, &lim)),
                            inverse: false,
                        }
                    }
                    "rec2100_surround" => {
                        need(1)?;
                        Op::Rec2100Surround {
                            gamma: p[0] as f32,
                            inverse: false,
                        }
                    }
                    "rgb_to_hsv" => Op::RgbToHsv { inverse: false },
                    "lin_to_pq" => Op::Curve {
                        curve: crate::builtins::Curve::LinToPq,
                        inverse: false,
                    },
                    _ => bail!("unsupported FixedFunction style '{style}'"),
                });
            }
            "AllocationTransform" => {
                let vars = get_fs(t, "vars")?.unwrap_or_default();
                let lg2 = get_str(t, "allocation").is_some_and(|a| a.eq_ignore_ascii_case("lg2"));
                let (lo, hi) = match vars.len() {
                    0 | 1 if lg2 => (-10., 6.),
                    0 | 1 => (0., 1.),
                    _ => (vars[0], vars[1]),
                };
                if lg2 {
                    let mut p = LogParams::PLAIN;
                    p.lin_offset = vars.get(2).copied().unwrap_or(0.);
                    ops.push(Op::Log {
                        base: 2.,
                        p: [p; 3],
                        inverse: false,
                    });
                }
                let s = 1. / (hi - lo);
                ops.push(Op::scale_offset([s; 3], [-lo * s; 3]));
            }
            "FileTransform" => {
                let src = get_str(t, "src").ok_or_else(|| anyhow!("FileTransform without src"))?;
                let interp = get_str(t, "interpolation")
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                for op in self.file_ops(&src)?.iter() {
                    let mut op = op.clone();
                    if let Op::Lut3D { tetra, .. } = &mut op {
                        match interp.as_str() {
                            "tetrahedral" | "best" => *tetra = true,
                            "linear" => *tetra = false,
                            // ponytail: "nearest"/"cubic" fall back to the file's default.
                            _ => {}
                        }
                    }
                    ops.push(op);
                }
            }
            "BuiltinTransform" => {
                ops.extend(builtin_ops(&get_str(t, "style").unwrap_or_default())?)
            }
            other => bail!("unsupported transform type '{other}'"),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // OCIO v1 syntax: to_reference, display views with looks, LUT from the search path.
    const V1: &str = r#"ocio_profile_version: 1
search_path: luts
roles:
  scene_linear: linear
  color_picking: curved
displays:
  sRGB:
    - !<View> {name: Film, colorspace: curved, looks: +grade}
    - !<View> {name: Raw, colorspace: raw}
looks:
  - !<Look>
    name: grade
    process_space: linear
    transform: !<CDLTransform> {slope: [2, 2, 2]}
colorspaces:
  - !<ColorSpace>
    name: linear
  - !<ColorSpace>
    name: raw
    isdata: true
  - !<ColorSpace>
    name: curved
    to_reference: !<FileTransform> {src: square.spi1d, interpolation: linear}
"#;

    #[test]
    fn v1_config() {
        let config = Config::from_str(V1, |p| {
            ensure!(p == "luts/square.spi1d", "unexpected path {p}");
            Ok(b"Version 1\nFrom 0 1\nLength 3\nComponents 1\n{\n0\n0.25\n1\n}\n".to_vec())
        })
        .unwrap();
        assert_eq!(config.default_display(), Some("sRGB"));
        assert_eq!(config.views("sRGB"), ["Film", "Raw"]);
        assert_eq!(config.color_space("color_picking").unwrap().name, "curved");

        // linear 0.125 -> grade x2 -> 0.25 -> inverse of the curve -> 0.5
        let mut p = [0.125; 3];
        config
            .display_processor("scene_linear", "sRGB", "Film", None)
            .unwrap()
            .apply_rgb(&mut p);
        assert!((p[0] - 0.5).abs() < 1e-6, "{p:?}");
        // Looks can be turned off: 0.125 -> inverse of the curve -> 0.25
        let mut p = [0.125; 3];
        config
            .display_processor("linear", "sRGB", "Film", Some(""))
            .unwrap()
            .apply_rgb(&mut p);
        assert!((p[0] - 0.25).abs() < 1e-6, "{p:?}");
        assert!(
            config
                .display_processor("linear", "sRGB", "Raw", None)
                .unwrap()
                .is_noop()
        );
    }

    #[test]
    fn verbatim_tags_become_local() {
        assert_eq!(
            local_tags("a: !<View> {x: \"!<not a tag\"}"),
            "a: !View {x: \"!<not a tag\"}"
        );
    }
}
