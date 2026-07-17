use std::time::Duration;

pub const SUPABASE_URL: &str = "https://isltexsxpysxqewjsryv.supabase.co";
/// Publishable key — public by design (ships in the app). The env
/// override always wins, which is also how the mock-server tests run.
pub const SUPABASE_PUBLISHABLE_KEY: &str = "sb_publishable_zKxcgZ9z7S4xpwd68Pv1bA_iNbv6oyq";

pub const OFFLINE_GRACE_DAYS: i64 = 30;
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(4);
pub const OVERALL_TIMEOUT: Duration = Duration::from_secs(8);

pub fn supabase_url() -> String {
    std::env::var("CLEOPHIS_SUPABASE_URL").unwrap_or_else(|_| SUPABASE_URL.to_string())
}

pub fn supabase_key() -> String {
    std::env::var("CLEOPHIS_SUPABASE_KEY").unwrap_or_else(|_| SUPABASE_PUBLISHABLE_KEY.to_string())
}
