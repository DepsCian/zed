use super::AvailableModel;

pub const PROVIDER_ID: language_model::LanguageModelProviderId =
    language_model::LanguageModelProviderId::new("kiro");
pub const PROVIDER_NAME: language_model::LanguageModelProviderName =
    language_model::LanguageModelProviderName::new("Kiro AI");

pub const DEFAULT_REGION: &str = "us-east-1";
pub const SCOPES: &[&str] = &[
    "codewhisperer:completions",
    "codewhisperer:analysis",
    "codewhisperer:conversations",
];

pub const AWS_BUILDER_ID_URL: &str = "https://view.awsapps.com/start";
pub const AVAILABLE_REGIONS: &[(&str, &str)] = &[
    ("us-east-1", "US East (N. Virginia)"),
    ("eu-central-1", "EU (Frankfurt)"),
];

#[derive(Default, Debug, Clone, PartialEq)]
pub struct KiroSettings {
    pub region: String,
    pub available_models: Vec<AvailableModel>,
}
