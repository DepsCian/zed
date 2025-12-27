use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserContext {
    pub ide_category: IdeCategory,
    pub operating_system: OperatingSystem,
    pub product: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum IdeCategory {
    VsCode,
    JetBrains,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OperatingSystem {
    Linux,
    Windows,
    Macos,
}

impl UserContext {
    pub fn for_zed() -> Self {
        Self {
            ide_category: IdeCategory::VsCode,
            operating_system: Self::detect_os(),
            product: "Zed".to_string(),
        }
    }

    fn detect_os() -> OperatingSystem {
        #[cfg(target_os = "linux")]
        return OperatingSystem::Linux;
        #[cfg(target_os = "macos")]
        return OperatingSystem::Macos;
        #[cfg(target_os = "windows")]
        return OperatingSystem::Windows;
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        return OperatingSystem::Linux;
    }
}
