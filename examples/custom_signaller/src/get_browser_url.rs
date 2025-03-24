pub fn get_browser_url_hash() -> Option<String> {
    // Get window.location object
    let window = web_sys::window().expect("no global `window` exists");
    let location = window.location();

    // Get the hash part of the URL (including the # symbol)
    let hash = location.hash().ok()?;

    // Remove the leading # symbol and return the remaining string
    Some(hash.trim_start_matches('#').to_string())
}
