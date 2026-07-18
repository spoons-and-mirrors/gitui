use std::{
    collections::HashSet,
    env, fs, io,
    path::{Path, PathBuf},
};

use ratatui::style::Color;
use serde_json::{Map, Value};

#[path = "theme_builtins.rs"]
mod theme_builtins;

pub const DEFAULT_THEME_NAME: &str = "catppuccin-macchiato";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    Dark,
    Light,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeSource {
    BuiltIn,
    User(PathBuf),
    Project(PathBuf),
    Fallback,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedTheme {
    pub name: String,
    pub source: ThemeSource,
    pub palette: Palette,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub canvas: Color,
    pub panel: Color,
    pub surface_alt: Color,
    pub raised: Color,
    pub selected: Color,
    pub inactive_selected: Color,
    pub ink: Color,
    pub muted: Color,
    pub faint: Color,
    pub accent: Color,
    pub purple: Color,
    pub green: Color,
    pub yellow: Color,
    pub red: Color,
    pub cyan: Color,
    pub orange: Color,
    pub add_bg: Color,
    pub remove_bg: Color,
    pub graph_colors: [Color; 8],
}

impl Default for Palette {
    fn default() -> Self {
        default_palette()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemePaths {
    pub config_dir: PathBuf,
    pub state_file: PathBuf,
    pub cwd: PathBuf,
}

impl ThemePaths {
    #[allow(dead_code)]
    pub fn new(
        config_dir: impl Into<PathBuf>,
        state_file: impl Into<PathBuf>,
        cwd: impl Into<PathBuf>,
    ) -> Self {
        Self {
            config_dir: config_dir.into(),
            state_file: state_file.into(),
            cwd: cwd.into(),
        }
    }

    pub fn discover() -> io::Result<Self> {
        let home = env::var_os("HOME").map(PathBuf::from);
        let config_root = env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|path| path.join(".config")))
            .unwrap_or_default();
        let state_root = env::var_os("XDG_STATE_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| home.map(|path| path.join(".local/state")))
            .unwrap_or_default();

        Ok(Self {
            config_dir: config_root.join("opencode"),
            state_file: state_root.join("opencode/kv.json"),
            cwd: env::current_dir()?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeLoader {
    paths: ThemePaths,
    appearance: Appearance,
}

impl ThemeLoader {
    pub fn new(paths: ThemePaths) -> Self {
        Self {
            paths,
            appearance: Appearance::Dark,
        }
    }

    #[must_use]
    pub fn appearance(mut self, appearance: Appearance) -> Self {
        self.appearance = appearance;
        self
    }

    #[must_use]
    pub fn load(&self) -> LoadedTheme {
        let Ok(selection) = selected_theme(&self.paths) else {
            return fallback_theme();
        };
        let Some(name) = selection else {
            return fallback_theme();
        };

        match find_custom_theme(&self.paths, &name) {
            Some((path, source)) => load_custom_palette(&path, self.appearance)
                .map(|palette| LoadedTheme {
                    name,
                    source,
                    palette,
                })
                .unwrap_or_else(|()| fallback_theme()),
            None => theme_builtins::palette(&name, self.appearance)
                .map(|palette| LoadedTheme {
                    name,
                    source: ThemeSource::BuiltIn,
                    palette,
                })
                .unwrap_or_else(fallback_theme),
        }
    }
}

#[must_use]
pub fn load_theme() -> LoadedTheme {
    let Ok(paths) = ThemePaths::discover() else {
        return fallback_theme();
    };
    let appearance = selected_appearance(&paths);
    ThemeLoader::new(paths).appearance(appearance).load()
}

fn default_palette() -> Palette {
    theme_builtins::palette(DEFAULT_THEME_NAME, Appearance::Dark)
        .expect("default theme is in the generated built-in registry")
}

fn fallback_theme() -> LoadedTheme {
    LoadedTheme {
        name: DEFAULT_THEME_NAME.to_owned(),
        source: ThemeSource::Fallback,
        palette: default_palette(),
    }
}

fn selected_theme(paths: &ThemePaths) -> Result<Option<String>, ()> {
    for name in ["tui.json", "tui.jsonc"] {
        let path = paths.config_dir.join(name);
        if !path.is_file() {
            continue;
        }
        let document = parse_json_file(&path)?;
        let document = document.as_object().ok_or(())?;
        if let Some(theme) = document.get("theme") {
            return theme
                .as_str()
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .map(Some)
                .ok_or(());
        }
    }

    if !paths.state_file.is_file() {
        return Ok(None);
    }
    let document = parse_json_file(&paths.state_file)?;
    let document = document.as_object().ok_or(())?;
    match document.get("theme") {
        Some(value) => value
            .as_str()
            .filter(|theme| !theme.is_empty())
            .map(str::to_owned)
            .map(Some)
            .ok_or(()),
        None => Ok(None),
    }
}

fn selected_appearance(paths: &ThemePaths) -> Appearance {
    let Ok(document) = parse_json_file(&paths.state_file) else {
        return Appearance::Dark;
    };
    let Some(document) = document.as_object() else {
        return Appearance::Dark;
    };
    let mode = document
        .get("theme_mode_lock")
        .or_else(|| document.get("theme_mode"))
        .and_then(Value::as_str);
    if mode == Some("light") {
        Appearance::Light
    } else {
        Appearance::Dark
    }
}

fn find_custom_theme(paths: &ThemePaths, name: &str) -> Option<(PathBuf, ThemeSource)> {
    let file_name = theme_file_name(name)?;
    for ancestor in paths.cwd.ancestors() {
        let path = ancestor.join(".opencode/themes").join(&file_name);
        if path.is_file() {
            return Some((path.clone(), ThemeSource::Project(path)));
        }
    }

    let path = paths.config_dir.join("themes").join(file_name);
    path.is_file()
        .then(|| (path.clone(), ThemeSource::User(path)))
}

fn theme_file_name(name: &str) -> Option<String> {
    (!name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\'])
        && !name.contains('\0'))
    .then(|| format!("{name}.json"))
}

fn load_custom_palette(path: &Path, appearance: Appearance) -> Result<Palette, ()> {
    let document = parse_json_file(path)?;
    let defs = match document.get("defs") {
        Some(value) => value.as_object().cloned().ok_or(())?,
        None => Map::new(),
    };
    let theme = document.get("theme").and_then(Value::as_object).ok_or(())?;
    Resolver {
        defs: &defs,
        theme,
        appearance,
    }
    .palette()
}

struct Resolver<'a> {
    defs: &'a Map<String, Value>,
    theme: &'a Map<String, Value>,
    appearance: Appearance,
}

impl Resolver<'_> {
    fn palette(&self) -> Result<Palette, ()> {
        Ok(Palette {
            canvas: self.key("background")?,
            panel: self.key("backgroundPanel")?,
            surface_alt: self.key("backgroundElement")?,
            raised: self.key("border")?,
            selected: self.key("borderActive")?,
            inactive_selected: self.key("border")?,
            ink: self.key("text")?,
            muted: self.key("textMuted")?,
            faint: self.key("borderSubtle")?,
            accent: self.key("primary")?,
            purple: self.key("secondary")?,
            green: self.key("success")?,
            yellow: self.key("warning")?,
            red: self.key("error")?,
            cyan: self.key("info")?,
            orange: self.key("diffHunkHeader")?,
            add_bg: self.key("diffAddedBg")?,
            remove_bg: self.key("diffRemovedBg")?,
            graph_colors: [
                self.key("primary")?,
                self.key("secondary")?,
                self.key("success")?,
                self.key("warning")?,
                self.key("error")?,
                self.key("info")?,
                self.key("diffHunkHeader")?,
                self.key("accent")?,
            ],
        })
    }

    fn key(&self, name: &str) -> Result<Color, ()> {
        let value = self.theme.get(name).ok_or(())?;
        self.value(value, &mut HashSet::new())
    }

    fn value(&self, value: &Value, resolving: &mut HashSet<String>) -> Result<Color, ()> {
        match value {
            Value::String(value) => self.string(value, resolving),
            Value::Number(value) => value
                .as_u64()
                .and_then(|value| u8::try_from(value).ok())
                .map(Color::Indexed)
                .ok_or(()),
            Value::Object(variants) => {
                let key = match self.appearance {
                    Appearance::Dark => "dark",
                    Appearance::Light => "light",
                };
                self.value(variants.get(key).ok_or(())?, resolving)
            }
            _ => Err(()),
        }
    }

    fn string(&self, value: &str, resolving: &mut HashSet<String>) -> Result<Color, ()> {
        if matches!(value, "none" | "transparent") {
            return Ok(Color::Reset);
        }
        if let Some(color) = parse_hex(value) {
            return Ok(color);
        }

        let reference = self
            .defs
            .get(value)
            .or_else(|| self.theme.get(value))
            .ok_or(())?;
        if !resolving.insert(value.to_owned()) {
            return Err(());
        }
        let color = self.value(reference, resolving);
        resolving.remove(value);
        color
    }
}

fn parse_hex(value: &str) -> Option<Color> {
    let value = value.strip_prefix('#')?;
    let (red, green, blue, alpha) = match value.len() {
        3 => (
            u8::from_str_radix(&value[0..1], 16).ok()? * 17,
            u8::from_str_radix(&value[1..2], 16).ok()? * 17,
            u8::from_str_radix(&value[2..3], 16).ok()? * 17,
            None,
        ),
        4 => (
            u8::from_str_radix(&value[0..1], 16).ok()? * 17,
            u8::from_str_radix(&value[1..2], 16).ok()? * 17,
            u8::from_str_radix(&value[2..3], 16).ok()? * 17,
            Some(u8::from_str_radix(&value[3..4], 16).ok()? * 17),
        ),
        6 => (
            u8::from_str_radix(&value[0..2], 16).ok()?,
            u8::from_str_radix(&value[2..4], 16).ok()?,
            u8::from_str_radix(&value[4..6], 16).ok()?,
            None,
        ),
        8 => (
            u8::from_str_radix(&value[0..2], 16).ok()?,
            u8::from_str_radix(&value[2..4], 16).ok()?,
            u8::from_str_radix(&value[4..6], 16).ok()?,
            Some(u8::from_str_radix(&value[6..8], 16).ok()?),
        ),
        _ => return None,
    };
    if alpha == Some(0) {
        Some(Color::Reset)
    } else {
        Some(Color::Rgb(red, green, blue))
    }
}

fn parse_json_file(path: &Path) -> Result<Value, ()> {
    let input = fs::read_to_string(path).map_err(|_| ())?;
    let input = normalize_jsonc(&input)?;
    serde_json::from_str(&input).map_err(|_| ())
}

fn normalize_jsonc(input: &str) -> Result<String, ()> {
    let chars: Vec<char> = input.chars().collect();
    let mut output = String::with_capacity(input.len());
    let mut index = 0;
    let mut in_string = false;
    let mut escaped = false;

    while index < chars.len() {
        let character = chars[index];
        if in_string {
            output.push(character);
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            index += 1;
            continue;
        }

        if character == '"' {
            in_string = true;
            output.push(character);
            index += 1;
        } else if character == '/' && chars.get(index + 1) == Some(&'/') {
            index += 2;
            while index < chars.len() && chars[index] != '\n' {
                index += 1;
            }
        } else if character == '/' && chars.get(index + 1) == Some(&'*') {
            index += 2;
            let mut closed = false;
            while index + 1 < chars.len() {
                if chars[index] == '*' && chars[index + 1] == '/' {
                    index += 2;
                    closed = true;
                    break;
                }
                if chars[index] == '\n' {
                    output.push('\n');
                }
                index += 1;
            }
            if !closed {
                return Err(());
            }
        } else {
            output.push(character);
            index += 1;
        }
    }

    let chars: Vec<char> = output.chars().collect();
    let mut normalized = String::with_capacity(output.len());
    in_string = false;
    escaped = false;
    for (index, character) in chars.iter().copied().enumerate() {
        if in_string {
            normalized.push(character);
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        if character == '"' {
            in_string = true;
            normalized.push(character);
            continue;
        }
        if character == ','
            && chars[index + 1..]
                .iter()
                .find(|next| !next.is_whitespace())
                .is_some_and(|next| matches!(next, '}' | ']'))
        {
            continue;
        }
        normalized.push(character);
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn paths(temp: &TempDir) -> ThemePaths {
        ThemePaths::new(
            temp.path().join("config/opencode"),
            temp.path().join("state/opencode/kv.json"),
            temp.path().join("work/repository"),
        )
    }

    fn write(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().expect("test path has a parent")).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn custom_theme(primary: &str) -> String {
        format!(
            r##"{{
                "defs": {{
                    "base": "#010203",
                    "panel": {{ "dark": "#040506", "light": "#a0a1a2" }},
                    "shared": "base"
                }},
                "theme": {{
                    "primary": {primary},
                    "secondary": "#101112",
                    "accent": "#131415",
                    "error": "#161718",
                    "warning": "#191a1b",
                    "success": "#1c1d1e",
                    "info": "#1f2021",
                    "text": "shared",
                    "textMuted": 42,
                    "background": "transparent",
                    "backgroundPanel": "panel",
                    "backgroundElement": "#252627",
                    "border": "#28292a",
                    "borderActive": "#2b2c2d",
                    "borderSubtle": "#2e2f30",
                    "diffHunkHeader": "#313233",
                    "diffAddedBg": "#343536",
                    "diffRemovedBg": "#373839"
                }}
            }}"##
        )
    }

    #[test]
    fn unavailable_selection_uses_exact_default() {
        let temp = TempDir::new().unwrap();
        let loaded = ThemeLoader::new(paths(&temp)).load();

        assert_eq!(loaded.name, DEFAULT_THEME_NAME);
        assert_eq!(loaded.source, ThemeSource::Fallback);
        assert_eq!(loaded.palette, default_palette());
        assert_eq!(loaded.palette.accent, Color::Rgb(0x8a, 0xad, 0xf4));
        assert_eq!(loaded.palette.canvas, Color::Rgb(0x24, 0x27, 0x3a));
        assert_eq!(loaded.palette.add_bg, Color::Rgb(0x29, 0x34, 0x2b));
    }

    #[test]
    fn kv_selects_registered_builtin() {
        let temp = TempDir::new().unwrap();
        let paths = paths(&temp);
        write(&paths.state_file, r#"{"theme":"catppuccin-macchiato"}"#);

        let loaded = ThemeLoader::new(paths).load();

        assert_eq!(loaded.source, ThemeSource::BuiltIn);
        assert_eq!(loaded.palette, default_palette());
    }

    #[test]
    fn kv_theme_mode_lock_selects_light_appearance() {
        let temp = TempDir::new().unwrap();
        let paths = paths(&temp);
        write(
            &paths.state_file,
            r#"{"theme":"catppuccin","theme_mode_lock":"light"}"#,
        );

        assert_eq!(selected_appearance(&paths), Appearance::Light);
    }

    #[test]
    fn every_official_builtin_resolves_with_distinct_representatives() {
        const NAMES: &[&str] = &[
            "aura",
            "ayu",
            "carbonfox",
            "catppuccin-frappe",
            "catppuccin-macchiato",
            "catppuccin",
            "cobalt2",
            "cursor",
            "dracula",
            "everforest",
            "flexoki",
            "github",
            "gruvbox",
            "kanagawa",
            "lucent-orng",
            "material",
            "matrix",
            "mercury",
            "monokai",
            "nightowl",
            "nord",
            "one-dark",
            "opencode",
            "orng",
            "osaka-jade",
            "palenight",
            "rosepine",
            "solarized",
            "synthwave84",
            "tokyonight",
            "vercel",
            "vesper",
            "zenburn",
        ];
        assert_eq!(theme_builtins::names().collect::<Vec<_>>(), NAMES);

        let temp = TempDir::new().unwrap();
        let paths = paths(&temp);
        let mut representatives = Vec::new();
        for name in NAMES {
            write(&paths.state_file, &format!(r#"{{"theme":"{name}"}}"#));
            let dark = ThemeLoader::new(paths.clone()).load();
            let light = ThemeLoader::new(paths.clone())
                .appearance(Appearance::Light)
                .load();

            assert_eq!(dark.name, *name);
            assert_eq!(dark.source, ThemeSource::BuiltIn);
            assert_eq!(light.source, ThemeSource::BuiltIn);
            if matches!(*name, "catppuccin-macchiato" | "gruvbox" | "tokyonight") {
                representatives.push(dark.palette.accent);
            }
        }

        assert_eq!(representatives.len(), 3);
        assert_ne!(representatives[0], representatives[1]);
        assert_ne!(representatives[0], representatives[2]);
        assert_ne!(representatives[1], representatives[2]);
    }

    #[test]
    fn jsonc_config_overrides_kv_and_preserves_comment_like_strings() {
        let temp = TempDir::new().unwrap();
        let paths = paths(&temp);
        write(&paths.state_file, r#"{"theme":"catppuccin-macchiato"}"#);
        write(
            &paths.config_dir.join("tui.jsonc"),
            r#"{
                "$schema": "https://opencode.ai/tui.json", // comment
                "theme": "custom", /* another comment */
            }"#,
        );
        let theme_path = paths.config_dir.join("themes/custom.json");
        write(&theme_path, &custom_theme(r##""#abcdef""##));

        let loaded = ThemeLoader::new(paths).load();

        assert_eq!(loaded.source, ThemeSource::User(theme_path));
        assert_eq!(loaded.palette.accent, Color::Rgb(0xab, 0xcd, 0xef));
    }

    #[test]
    fn nearest_project_theme_resolves_refs_variants_ansi_and_transparency() {
        let temp = TempDir::new().unwrap();
        let paths = paths(&temp);
        write(&paths.state_file, r#"{"theme":"custom"}"#);
        let user_theme = paths.config_dir.join("themes/custom.json");
        write(&user_theme, &custom_theme(r##""#ffffff""##));
        let ancestor_theme = temp.path().join("work/.opencode/themes/custom.json");
        write(&ancestor_theme, &custom_theme(r##""#999999""##));
        let project_theme = paths.cwd.join(".opencode/themes/custom.json");
        write(&project_theme, &custom_theme(r##""#abcdef""##));

        let dark = ThemeLoader::new(paths.clone()).load();
        let light = ThemeLoader::new(paths).appearance(Appearance::Light).load();

        assert_eq!(dark.source, ThemeSource::Project(project_theme.clone()));
        assert_eq!(dark.palette.canvas, Color::Reset);
        assert_eq!(dark.palette.ink, Color::Rgb(1, 2, 3));
        assert_eq!(dark.palette.muted, Color::Indexed(42));
        assert_eq!(dark.palette.panel, Color::Rgb(4, 5, 6));
        assert_eq!(dark.palette.accent, Color::Rgb(0xab, 0xcd, 0xef));
        assert_eq!(light.source, ThemeSource::Project(project_theme));
        assert_eq!(light.palette.panel, Color::Rgb(0xa0, 0xa1, 0xa2));
    }

    #[test]
    fn malformed_config_or_custom_theme_uses_exact_default() {
        let temp = TempDir::new().unwrap();
        let mut paths = paths(&temp);
        write(&paths.state_file, r#"{"theme":"catppuccin-macchiato"}"#);
        write(&paths.config_dir.join("tui.jsonc"), "{/* unclosed");
        assert_eq!(ThemeLoader::new(paths.clone()).load(), fallback_theme());

        fs::remove_file(paths.config_dir.join("tui.jsonc")).unwrap();
        write(&paths.state_file, r#"{"theme":"broken"}"#);
        paths.cwd = temp.path().join("work");
        write(
            &paths.cwd.join(".opencode/themes/broken.json"),
            &custom_theme(r#""missing-reference""#),
        );
        assert_eq!(ThemeLoader::new(paths).load(), fallback_theme());
    }

    #[test]
    fn cyclic_references_use_exact_default() {
        let temp = TempDir::new().unwrap();
        let paths = paths(&temp);
        write(&paths.state_file, r#"{"theme":"cyclic"}"#);
        write(
            &paths.config_dir.join("themes/cyclic.json"),
            &custom_theme(r#""primary""#),
        );

        assert_eq!(ThemeLoader::new(paths).load(), fallback_theme());
    }

    #[test]
    fn same_named_defs_and_upstream_hex_forms_resolve() {
        let document = serde_json::json!({
            "defs": { "primary": "#abc" },
            "theme": { "primary": "primary" }
        });
        let resolver = Resolver {
            defs: document["defs"].as_object().unwrap(),
            theme: document["theme"].as_object().unwrap(),
            appearance: Appearance::Dark,
        };

        assert_eq!(resolver.key("primary"), Ok(Color::Rgb(0xaa, 0xbb, 0xcc)));
        assert_eq!(parse_hex("#1230"), Some(Color::Reset));
        assert_eq!(parse_hex("#12345680"), Some(Color::Rgb(0x12, 0x34, 0x56)));
    }
}
