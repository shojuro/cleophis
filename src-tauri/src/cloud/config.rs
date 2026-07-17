use std::time::Duration;

pub const SUPABASE_URL: &str = "https://isltexsxpysxqewjsryv.supabase.co";
/// Publishable (anon) key — public by design; the real value is filled in
/// once retrieved from the project dashboard (Task A8 gate). The env
/// override always wins, which is also how the mock-server tests run.
pub const SUPABASE_PUBLISHABLE_KEY: &str = "";

pub const OFFLINE_GRACE_DAYS: i64 = 30;
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(4);
pub const OVERALL_TIMEOUT: Duration = Duration::from_secs(8);

pub fn supabase_url() -> String {
    std::env::var("CLEOPHIS_SUPABASE_URL").unwrap_or_else(|_| SUPABASE_URL.to_string())
}

pub fn supabase_key() -> String {
    std::env::var("CLEOPHIS_SUPABASE_KEY").unwrap_or_else(|_| SUPABASE_PUBLISHABLE_KEY.to_string())
}
