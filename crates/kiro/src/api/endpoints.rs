pub fn get_endpoint(region: &str) -> &'static str {
    match region {
        "us-east-1" => "https://q.us-east-1.amazonaws.com",
        "eu-central-1" => "https://q.eu-central-1.amazonaws.com",
        _ => "https://q.us-east-1.amazonaws.com",
    }
}

pub fn get_oidc_endpoint(region: &str) -> String {
    format!("https://oidc.{}.amazonaws.com", region)
}
