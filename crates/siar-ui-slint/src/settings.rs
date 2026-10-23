//! User-facing appearance settings (ui-ux-22 §17 theme mode, §49 density,
//! §37 UI scale). Plain `key=value` text: trivially inspectable, no
//! serialization dependency, and a damaged or hand-edited file degrades to
//! defaults instead of failing startup.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeChoice {
    #[default]
    System,
    Light,
    Dark,
    HighContrast,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DensityChoice {
    #[default]
    Comfortable,
    Compact,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UiSettings {
    pub theme: ThemeChoice,
    pub density: DensityChoice,
    /// Clamped to [`UiSettings::MIN_SCALE`], [`UiSettings::MAX_SCALE`].
    pub scale: f32,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::System,
            density: DensityChoice::Comfortable,
            scale: 1.0,
        }
    }
}

impl UiSettings {
    pub const MIN_SCALE: f32 = 0.8;
    pub const MAX_SCALE: f32 = 1.5;

    pub fn clamp_scale(s: f32) -> f32 {
        if s.is_finite() {
            s.clamp(Self::MIN_SCALE, Self::MAX_SCALE)
        } else {
            1.0
        }
    }

    pub fn to_text(self) -> String {
        let theme = match self.theme {
            ThemeChoice::System => "system",
            ThemeChoice::Light => "light",
            ThemeChoice::Dark => "dark",
            ThemeChoice::HighContrast => "high-contrast",
        };
        let density = match self.density {
            DensityChoice::Comfortable => "comfortable",
            DensityChoice::Compact => "compact",
        };
        format!(
            "theme={theme}\ndensity={density}\nscale={:.2}\n",
            Self::clamp_scale(self.scale)
        )
    }

    /// Unknown keys and values are ignored field by field, so a file
    /// written by a newer build still yields the fields this build knows.
    pub fn from_text(text: &str) -> Self {
        let mut out = Self::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match (key.trim(), value.trim()) {
                ("theme", "system") => out.theme = ThemeChoice::System,
                ("theme", "light") => out.theme = ThemeChoice::Light,
                ("theme", "dark") => out.theme = ThemeChoice::Dark,
                ("theme", "high-contrast") => out.theme = ThemeChoice::HighContrast,
                ("density", "comfortable") => out.density = DensityChoice::Comfortable,
                ("density", "compact") => out.density = DensityChoice::Compact,
                ("scale", v) => {
                    if let Ok(s) = v.parse::<f32>() {
                        out.scale = Self::clamp_scale(s);
                    }
                }
                _ => {}
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let s = UiSettings {
            theme: ThemeChoice::HighContrast,
            density: DensityChoice::Compact,
            scale: 1.15,
        };
        assert_eq!(UiSettings::from_text(&s.to_text()), s);
    }

    #[test]
    fn garbage_degrades_to_defaults_per_field() {
        assert_eq!(
            UiSettings::from_text("\u{0}\u{1}not a settings file"),
            UiSettings::default()
        );
        let s = UiSettings::from_text("theme=dark\ndensity=sideways\nscale=abc\nfuture=1");
        assert_eq!(s.theme, ThemeChoice::Dark);
        assert_eq!(s.density, DensityChoice::Comfortable);
        assert_eq!(s.scale, 1.0);
    }

    #[test]
    fn scale_is_clamped_and_nan_safe() {
        assert_eq!(
            UiSettings::from_text("scale=99").scale,
            UiSettings::MAX_SCALE
        );
        assert_eq!(
            UiSettings::from_text("scale=0.01").scale,
            UiSettings::MIN_SCALE
        );
        assert_eq!(UiSettings::clamp_scale(f32::NAN), 1.0);
        assert_eq!(UiSettings::from_text("scale=NaN").scale, 1.0);
    }
}
