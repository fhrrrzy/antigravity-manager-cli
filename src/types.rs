use std::collections::HashMap;
use ratatui::style::Color;
use serde::{Deserialize, Serialize};

// Google OAuth Constants
pub const CLIENT_ID: &str = "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";
pub const CLIENT_SECRET: &str = "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";
pub const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

// Cooldown duration: 4 hours
pub const COOLDOWN_SECONDS: i64 = 14400;

// Redirect Port for Local Auth listener
pub const OAUTH_PORT: u16 = 14210;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub email: String,
    pub refresh_token: String,
    pub name: String,
    pub source: String,
    #[serde(default)]
    pub id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenCache {
    pub access_token: String,
    pub expiry_timestamp: i64,
    pub project_id: Option<String>,
    pub subscription_tier: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelQuota {
    pub name: String,
    pub percentage: i32,
    pub reset_time: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaBucket {
    pub bucket_id: String,
    pub window: String,
    pub remaining_fraction: f64,
    pub reset_time: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaGroup {
    pub display_name: String,
    pub buckets: Vec<QuotaBucket>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaData {
    pub subscription_tier: Option<String>,
    pub models: Vec<ModelQuota>,
    #[serde(default)]
    pub quota_groups: Option<Vec<QuotaGroup>>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ThemeType {
    KanagawaDragon,
    GruvboxDark,
    Nord,
    Dracula,
    OneDark,
    RetroMatrix,
    SolarizedDark,
    Catppuccin,
    RosePine,
    TokyoNight,
    AyuDark,
}

impl ThemeType {
    pub fn from_str(name: &str) -> Self {
        match name.to_lowercase().as_str() {
            "gruvbox dark" | "gruvbox" => ThemeType::GruvboxDark,
            "nord" => ThemeType::Nord,
            "dracula" => ThemeType::Dracula,
            "one dark" | "onedark" => ThemeType::OneDark,
            "retro matrix" | "matrix" => ThemeType::RetroMatrix,
            "solarized dark" | "solarized" => ThemeType::SolarizedDark,
            "catppuccin" | "catppuccin macchiato" => ThemeType::Catppuccin,
            "rose pine" | "rosepine" => ThemeType::RosePine,
            "tokyo night" | "tokyonight" => ThemeType::TokyoNight,
            "ayu dark" | "ayudark" => ThemeType::AyuDark,
            _ => ThemeType::KanagawaDragon,
        }
    }

    pub fn to_str(&self) -> &'static str {
        match self {
            ThemeType::KanagawaDragon => "Kanagawa Dragon",
            ThemeType::GruvboxDark => "Gruvbox Dark",
            ThemeType::Nord => "Nord",
            ThemeType::Dracula => "Dracula",
            ThemeType::OneDark => "One Dark",
            ThemeType::RetroMatrix => "Retro Matrix",
            ThemeType::SolarizedDark => "Solarized Dark",
            ThemeType::Catppuccin => "Catppuccin",
            ThemeType::RosePine => "Rose Pine",
            ThemeType::TokyoNight => "Tokyo Night",
            ThemeType::AyuDark => "Ayu Dark",
        }
    }

    pub fn get_palette(&self) -> ThemePalette {
        match self {
            ThemeType::KanagawaDragon => ThemePalette {
                name: "Kanagawa Dragon",
                bg: Color::Rgb(20, 20, 30),
                fg: Color::Rgb(220, 215, 186),
                border_active: Color::Rgb(122, 168, 159),
                border_inactive: Color::Rgb(84, 84, 96),
                selection_bg: Color::Rgb(42, 42, 53),
                green_success: Color::Rgb(138, 154, 134),
                yellow_warning: Color::Rgb(196, 178, 138),
                red_danger: Color::Rgb(196, 116, 110),
                blue_reset_5h: Color::Rgb(139, 164, 177),
                violet_reset_weekly: Color::Rgb(147, 138, 169),
            },
            ThemeType::GruvboxDark => ThemePalette {
                name: "Gruvbox Dark",
                bg: Color::Rgb(40, 40, 40),
                fg: Color::Rgb(235, 219, 178),
                border_active: Color::Rgb(254, 128, 25),
                border_inactive: Color::Rgb(102, 92, 84),
                selection_bg: Color::Rgb(60, 56, 54),
                green_success: Color::Rgb(152, 151, 26),
                yellow_warning: Color::Rgb(250, 189, 47),
                red_danger: Color::Rgb(204, 36, 29),
                blue_reset_5h: Color::Rgb(131, 165, 152),
                violet_reset_weekly: Color::Rgb(211, 134, 155),
            },
            ThemeType::Nord => ThemePalette {
                name: "Nord",
                bg: Color::Rgb(46, 52, 64),
                fg: Color::Rgb(236, 239, 244),
                border_active: Color::Rgb(136, 192, 208),
                border_inactive: Color::Rgb(76, 86, 106),
                selection_bg: Color::Rgb(67, 76, 94),
                green_success: Color::Rgb(163, 190, 140),
                yellow_warning: Color::Rgb(235, 203, 139),
                red_danger: Color::Rgb(191, 97, 106),
                blue_reset_5h: Color::Rgb(129, 161, 193),
                violet_reset_weekly: Color::Rgb(180, 142, 173),
            },
            ThemeType::Dracula => ThemePalette {
                name: "Dracula",
                bg: Color::Rgb(40, 42, 54),
                fg: Color::Rgb(248, 248, 242),
                border_active: Color::Rgb(189, 147, 249),
                border_inactive: Color::Rgb(98, 114, 164),
                selection_bg: Color::Rgb(68, 71, 90),
                green_success: Color::Rgb(80, 250, 123),
                yellow_warning: Color::Rgb(241, 250, 140),
                red_danger: Color::Rgb(255, 85, 85),
                blue_reset_5h: Color::Rgb(139, 233, 253),
                violet_reset_weekly: Color::Rgb(255, 121, 198),
            },
            ThemeType::OneDark => ThemePalette {
                name: "One Dark",
                bg: Color::Rgb(40, 44, 52),
                fg: Color::Rgb(171, 178, 191),
                border_active: Color::Rgb(97, 175, 239),
                border_inactive: Color::Rgb(92, 99, 112),
                selection_bg: Color::Rgb(44, 50, 60),
                green_success: Color::Rgb(152, 195, 121),
                yellow_warning: Color::Rgb(229, 192, 123),
                red_danger: Color::Rgb(224, 108, 117),
                blue_reset_5h: Color::Rgb(86, 182, 194),
                violet_reset_weekly: Color::Rgb(198, 120, 221),
            },
            ThemeType::RetroMatrix => ThemePalette {
                name: "Retro Matrix",
                bg: Color::Rgb(0, 0, 0),
                fg: Color::Rgb(0, 255, 0),
                border_active: Color::Rgb(0, 255, 0),
                border_inactive: Color::Rgb(0, 100, 0),
                selection_bg: Color::Rgb(0, 50, 0),
                green_success: Color::Rgb(0, 255, 0),
                yellow_warning: Color::Rgb(0, 200, 0),
                red_danger: Color::Rgb(0, 150, 0),
                blue_reset_5h: Color::Rgb(0, 180, 0),
                violet_reset_weekly: Color::Rgb(0, 220, 0),
            },
            ThemeType::SolarizedDark => ThemePalette {
                name: "Solarized Dark",
                bg: Color::Rgb(7, 54, 66),
                fg: Color::Rgb(147, 161, 161),
                border_active: Color::Rgb(38, 139, 210),
                border_inactive: Color::Rgb(88, 110, 117),
                selection_bg: Color::Rgb(0, 43, 54),
                green_success: Color::Rgb(133, 153, 0),
                yellow_warning: Color::Rgb(181, 137, 0),
                red_danger: Color::Rgb(220, 50, 47),
                blue_reset_5h: Color::Rgb(42, 161, 152),
                violet_reset_weekly: Color::Rgb(108, 113, 196),
            },
            ThemeType::Catppuccin => ThemePalette {
                name: "Catppuccin",
                bg: Color::Rgb(36, 39, 58),
                fg: Color::Rgb(202, 211, 245),
                border_active: Color::Rgb(198, 160, 246),
                border_inactive: Color::Rgb(91, 96, 120),
                selection_bg: Color::Rgb(54, 58, 79),
                green_success: Color::Rgb(166, 218, 149),
                yellow_warning: Color::Rgb(238, 212, 159),
                red_danger: Color::Rgb(237, 135, 150),
                blue_reset_5h: Color::Rgb(139, 213, 202),
                violet_reset_weekly: Color::Rgb(245, 189, 230),
            },
            ThemeType::RosePine => ThemePalette {
                name: "Rose Pine",
                bg: Color::Rgb(25, 23, 36),
                fg: Color::Rgb(224, 222, 244),
                border_active: Color::Rgb(196, 167, 231),
                border_inactive: Color::Rgb(85, 81, 105),
                selection_bg: Color::Rgb(42, 40, 55),
                green_success: Color::Rgb(156, 207, 216),
                yellow_warning: Color::Rgb(246, 193, 119),
                red_danger: Color::Rgb(235, 188, 186),
                blue_reset_5h: Color::Rgb(156, 207, 216),
                violet_reset_weekly: Color::Rgb(196, 167, 231),
            },
            ThemeType::TokyoNight => ThemePalette {
                name: "Tokyo Night",
                bg: Color::Rgb(36, 40, 59),
                fg: Color::Rgb(192, 202, 245),
                border_active: Color::Rgb(122, 162, 247),
                border_inactive: Color::Rgb(86, 95, 137),
                selection_bg: Color::Rgb(47, 53, 79),
                green_success: Color::Rgb(158, 206, 106),
                yellow_warning: Color::Rgb(224, 175, 104),
                red_danger: Color::Rgb(247, 118, 142),
                blue_reset_5h: Color::Rgb(13, 185, 215),
                violet_reset_weekly: Color::Rgb(187, 154, 247),
            },
            ThemeType::AyuDark => ThemePalette {
                name: "Ayu Dark",
                bg: Color::Rgb(15, 20, 25),
                fg: Color::Rgb(230, 180, 80),
                border_active: Color::Rgb(255, 180, 84),
                border_inactive: Color::Rgb(62, 75, 89),
                selection_bg: Color::Rgb(36, 51, 64),
                green_success: Color::Rgb(127, 217, 98),
                yellow_warning: Color::Rgb(242, 151, 24),
                red_danger: Color::Rgb(240, 113, 120),
                blue_reset_5h: Color::Rgb(57, 186, 230),
                violet_reset_weekly: Color::Rgb(242, 89, 75),
            },
        }
    }
}

#[allow(dead_code)]
pub struct ThemePalette {
    pub name: &'static str,
    pub bg: Color,
    pub fg: Color,
    pub border_active: Color,
    pub border_inactive: Color,
    pub selection_bg: Color,
    pub green_success: Color,
    pub yellow_warning: Color,
    pub red_danger: Color,
    pub blue_reset_5h: Color,
    pub violet_reset_weekly: Color,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountHealth {
    #[serde(default)]
    pub consecutive_failures: u32,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub last_check_timestamp: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonAccountInfo {
    pub email: String,
    pub name: String,
    pub active: bool,
    pub source: String,
    pub consecutive_failures: u32,
    pub last_error: Option<String>,
    pub last_check_timestamp: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonQuotaOutput {
    pub email: String,
    pub subscription_tier: Option<String>,
    pub project_id: Option<String>,
    pub models: Vec<ModelQuota>,
    pub quota_groups: Option<Vec<QuotaGroup>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliCache {
    pub active_email: Option<String>,
    #[serde(default)]
    pub tokens: HashMap<String, TokenCache>,
    #[serde(default)]
    pub quotas: HashMap<String, QuotaData>,
    #[serde(default)]
    pub theme: Option<String>,
    #[serde(default)]
    pub health: HashMap<String, AccountHealth>,
}

#[derive(Clone, PartialEq)]
pub enum InputMode {
    Normal,
    AddAccount {
        email: String,
        refresh_token: String,
        active_field: usize, // 0 for Email, 1 for Refresh Token
        error_message: Option<String>,
    },
    OAuthLogin {
        auth_url: String,
    },
    ConfirmDelete {
        email: String,
    },
}

pub enum AddAccountAction {
    Cancel,
    CycleField,
    InputChar(char),
    Backspace,
    Submit,
}

pub enum AppEvent {
    Key(crossterm::event::KeyEvent),
    Mouse(crossterm::event::MouseEvent),
    Tick,
    Progress(String),
    NetworkSuccess(NetworkResult),
    NetworkError(String),
}

pub enum NetworkResult {
    QuotaRefreshed {
        email: String,
        quota: QuotaData,
        project_id: Option<String>,
    },
    WarmupComplete {
        email: String,
        warmup_count: usize,
        skipped_count: usize,
        logs: Vec<String>,
    },
    SwitchComplete {
        email: String,
        keyring_success: bool,
    },
    AddAccountComplete {
        new_account: Account,
    },
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SortMode {
    Email,
    Gemini5h,
    GeminiWeekly,
    Claude5h,
    ClaudeWeekly,
}

impl SortMode {
    pub fn to_str(&self) -> &str {
        match self {
            SortMode::Email => "Email",
            SortMode::Gemini5h => "Gemini 5h",
            SortMode::GeminiWeekly => "Gemini Weekly",
            SortMode::Claude5h => "Claude 5h",
            SortMode::ClaudeWeekly => "Claude Weekly",
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Focus {
    Accounts,
    Breakdown,
}
