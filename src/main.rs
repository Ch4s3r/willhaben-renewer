use anyhow::{Context, Result};
use clap::Parser;
use log::{info, warn};
use thirtyfour::prelude::*;
use thirtyfour::{ChromeCapabilities, DesiredCapabilities};
use tokio::time::{sleep, Duration};
use std::process::{Stdio, Child};
use std::io::{self, Write};
use url::Url;

/// CLI arguments for the willhaben renewer automation.
#[derive(Debug, Parser)]
#[command(author, version, about = "Automate re-publishing expired willhaben adverts", long_about = None)]
struct Args {
    /// Username / email for willhaben login
    #[arg(long, env = "WILLHABEN_USERNAME")]
    username: String,

    /// Password for willhaben login
    #[arg(long, env = "WILLHABEN_PASSWORD")]
    password: String,

    /// WebDriver endpoint (chromedriver). Default assumes chromedriver is started locally.
    #[arg(long, default_value = "http://localhost:9515")] 
    webdriver: String,

    /// Headless mode
    #[arg(long, default_value_t = false)]
    headless: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    let args = Args::parse();

    info!("Starting automation");

    // Optionally auto-start chromedriver if pointing to localhost and not already listening.
    let mut chromedriver_child: Option<Child> = None;
    if should_autostart(&args.webdriver)? {
        info!("Spawning local chromedriver");
        chromedriver_child = Some(start_chromedriver()?);
        // Give it a moment to bind the port.
        sleep(Duration::from_secs(1)).await;
    }

    let caps = build_caps(args.headless)?;
    let driver = WebDriver::new(&args.webdriver, caps).await.context("connect to chromedriver")?;

    if let Err(e) = run(&driver, &args.username, &args.password).await {
        warn!("Run failed: {e:?}");
    }

    driver.quit().await.ok();
    if let Some(mut child) = chromedriver_child { let _ = child.kill(); }
    info!("Done");
    Ok(())
}

fn should_autostart(webdriver_url: &str) -> Result<bool> {
    let parsed = Url::parse(webdriver_url).context("parse webdriver url")?;
    let host = parsed.host_str().unwrap_or("");
    let is_local = matches!(host, "localhost" | "127.0.0.1" | "::1");
    if !is_local { return Ok(false); }
    // If chromedriver binary exists in PATH or current directory, we'll attempt to start it.
    Ok(true)
}

fn start_chromedriver() -> Result<Child> {
    // Try using chromedriver from PATH. Optionally allow CHROMEDRIVER_PATH env override.
    let bin = std::env::var("CHROMEDRIVER_PATH").ok().filter(|v| !v.is_empty());
    let program = bin.as_deref().unwrap_or("chromedriver");
    // Basic command; let user supply extra args via env if needed.
    let mut cmd = std::process::Command::new(program);
    cmd.arg("--port=9515")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .stdin(Stdio::null());
    let child = cmd.spawn().context("spawn chromedriver")?;
    Ok(child)
}

fn build_caps(headless: bool) -> Result<ChromeCapabilities> {
    let mut caps = DesiredCapabilities::chrome();
    let chrome_binary = which::which("google-chrome")
        .or_else(|_| which::which("google-chrome-stable"))
        .or_else(|_| which::which("chromium"))
        .context("No Chrome/Chromium binary found in PATH")?;
    caps.set_binary(
        chrome_binary
            .to_str()
            .context("Chrome binary path is not valid UTF-8")?,
    )?;
    if headless { caps.add_arg("--headless=new")?; }
    caps.add_arg("--no-sandbox")?;
    caps.add_arg("--disable-dev-shm-usage")?;
    caps.add_arg("--window-size=1920,1080")?;
    Ok(caps)
}

async fn run(driver: &WebDriver, username: &str, password: &str) -> Result<()> {
    login(driver, username, password).await?;
    process_expired(driver).await?;
    Ok(())
}

async fn login(driver: &WebDriver, username: &str, password: &str) -> Result<()> {
    info!("Opening homepage");
    driver.get("https://www.willhaben.at/").await?;
    maybe_accept_cookies(driver).await.ok();

    // Click login (Anmelden) button - attempt a few known selectors.
    info!("Opening login form");
    // Try header login button
    let login_button = find_any(driver, &[ 
        By::Css("a[data-testid='header-login']"),
        By::Css("button[data-testid='header-login']"),
        By::LinkText("Einloggen"),
        By::LinkText("Anmelden"),
    ], 8).await?;
    login_button.click().await?;

    // Wait for username/password fields
    let user_field = find_any(driver, &[By::Css("input[type='email']"), By::Id("username"), By::Name("username")], 10).await?;
    user_field.send_keys(username).await?;

    let pass_field = find_any(driver, &[By::Css("input[type='password']"), By::Id("password"), By::Name("password")], 5).await?;
    pass_field.send_keys(password).await?;

    // Submit (look for submit button)
    let submit = find_any(driver, &[By::Css("button[type='submit']"), By::Css("button[data-testid='login-submit']")], 5).await?;
    submit.click().await?;

    // Handle possible SMS OTP verification step (returns true if OTP flow was executed)
    let _otp_used = maybe_handle_otp(driver).await?;

    // Detect login completion by absence of login links/text ('Einloggen' or 'Anmelden').
    info!("Waiting until login links disappear (indicates authenticated session)...");
    let start = std::time::Instant::now();
    let timeout = Duration::from_secs(25);
    loop {
        let einloggen_visible = is_link_visible(driver, "Einloggen").await?;
        let anmelden_visible = is_link_visible(driver, "Anmelden").await?;
        if !einloggen_visible && !anmelden_visible { break; }
        if start.elapsed() > timeout { warn!("Login links still visible after timeout; continuing anyway"); break; }
        sleep(Duration::from_millis(400)).await;
    }
    info!("Login considered complete; navigating to expired adverts page");
    driver.get("https://www.willhaben.at/iad/myprofile/myadverts?statusFilters=EXPIRED").await.ok();
    Ok(())
}

async fn is_link_visible(driver: &WebDriver, link_text: &str) -> Result<bool> {
    match driver.find(By::LinkText(link_text)).await {
        Ok(el) => Ok(el.is_displayed().await.unwrap_or(false)),
        Err(_) => Ok(false),
    }
}

async fn maybe_handle_otp(driver: &WebDriver) -> Result<bool> {
    // Quick check if OTP page is present (look for first input or page title)
    if driver.find(By::Id("otp-input-1")).await.is_err() && driver.find(By::Id("kc-page-title")).await.is_err() {
        return Ok(false); // no OTP step
    }
    info!("OTP verification required. Prompting for SMS code...");
    // Try to display reference code to user
    if let Ok(ref_el) = driver.find(By::Id("referenceID")).await {
        if let Ok(Some(val)) = ref_el.attr("value").await { println!("Reference code: {val}"); }
    } else if let Ok(wrapper) = driver.find(By::Css(".wh-otp-reference-id-wrapper")).await {
        if let Ok(text) = wrapper.text().await { println!("{text}"); }
    }
    let code = prompt_for_code()?;
    // Fill each digit
    for (i, ch) in code.chars().enumerate() {
        let id = format!("otp-input-{}", i + 1);
        if let Ok(input) = driver.find(By::Id(&id)).await {
            input.clear().await.ok();
            input.send_keys(ch.to_string()).await.ok();
        }
    }
    // Click submit button
    if let Ok(btn) = driver.find(By::Id("submitOneTimePassword")).await { btn.click().await.ok(); }
    info!("Submitted OTP code");
    // Brief wait to allow session finalization
    sleep(Duration::from_secs(1)).await;
    Ok(true)
}

fn prompt_for_code() -> Result<String> {
    loop {
        print!("Enter 4-digit SMS code: ");
        io::stdout().flush().ok();
        let mut buf = String::new();
        io::stdin().read_line(&mut buf).context("read otp code")?;
        let trimmed = buf.trim();
        let digits: String = trimmed.chars().filter(|c| c.is_ascii_digit()).collect();
        if digits.len() >= 4 { return Ok(digits.chars().take(4).collect()); }
        eprintln!("Invalid code, please enter at least 4 digits");
    }
}

async fn process_expired(driver: &WebDriver) -> Result<()> {
    let expired_url = "https://www.willhaben.at/iad/myprofile/myadverts?statusFilters=EXPIRED";
    loop {
        info!("Loading expired adverts list");
        driver.get(expired_url).await?;
        sleep(Duration::from_secs(2)).await; // allow page to populate
        maybe_accept_cookies(driver).await.ok();

        // Find all buttons with text 'Neu veröffentlichen'
        let buttons = find_all_buttons_with_text(driver, "Neu veröffentlichen").await?;
        if buttons.is_empty() {
            info!("No more expired adverts to re-publish");
            break;
        }

        info!("Found {} adverts to re-publish this pass", buttons.len());
        for idx in 0..buttons.len() { // re-query each loop because DOM becomes stale after navigation
            driver.get(expired_url).await?;
            sleep(Duration::from_secs(1)).await;
            let buttons_now = find_all_buttons_with_text(driver, "Neu veröffentlichen").await?;
            if idx >= buttons_now.len() { break; }
            info!("Re-publishing advert {}/{}", idx + 1, buttons.len());
            buttons_now[idx].click().await?;
            republish_flow(driver).await?;
        }
    }
    Ok(())
}

async fn republish_flow(driver: &WebDriver) -> Result<()> {
    // Expect now on first page of renew flow. Click 'Weiter' twice, then 'Veröffentlichen'
    for step in 1..=2 {
        let weiter = wait_for_button_with_text(driver, "Weiter", 15).await?;
        weiter.click().await?;
        info!("Clicked Weiter step {step}");
        sleep(Duration::from_secs(1)).await;
    }
    let veroeffentlichen = wait_for_button_with_text(driver, "Veröffentlichen", 20).await?;
    veroeffentlichen.click().await?;
    info!("Clicked Veröffentlichen");
    // small wait for completion
    sleep(Duration::from_secs(2)).await;
    Ok(())
}

async fn maybe_accept_cookies(driver: &WebDriver) -> Result<()> {
    // Attempt quickly to find and click the explicit 'Cookies akzeptieren' button first.
    let selectors = [
        By::XPath("//button[contains(normalize-space(.), 'Cookies akzeptieren')]") ,
        By::Css("button#onetrust-accept-btn-handler"),
        By::XPath("//button[contains(normalize-space(.), 'Alle akzeptieren')]") ,
        By::XPath("//button[contains(normalize-space(.), 'Akzeptieren')]") ,
        By::Css("button[aria-label='Akzeptieren']"),
    ];
    if let Ok(el) = find_any(driver, &selectors, 5).await {
        if el.is_displayed().await.unwrap_or(false) { let _ = el.click().await; }
    }
    Ok(())
}

async fn find_all_buttons_with_text(driver: &WebDriver, text: &str) -> Result<Vec<WebElement>> {
    let xpath = format!("//button[contains(normalize-space(.), '{text}')] | //a[contains(@role,'button') and contains(normalize-space(.), '{text}')]" );
    let elems = driver.find_all(By::XPath(&xpath)).await?;
    Ok(elems)
}

async fn wait_for_button_with_text(driver: &WebDriver, text: &str, timeout_secs: u64) -> Result<WebElement> {
    wait_for_any(driver, &[By::XPath(&format!("//button[contains(normalize-space(.), '{text}')]")), By::XPath(&format!("//a[contains(@role,'button') and contains(normalize-space(.), '{text}')]"))], timeout_secs).await
}

async fn find_any(driver: &WebDriver, selectors: &[By], timeout_secs: u64) -> Result<WebElement> {
    let start = std::time::Instant::now();
    loop {
        for sel in selectors {
            if let Ok(el) = driver.find(sel.clone()).await { return Ok(el); }
        }
        if start.elapsed() > Duration::from_secs(timeout_secs) {
            anyhow::bail!("Element not found for any selector after {timeout_secs}s");
        }
        sleep(Duration::from_millis(300)).await;
    }
}

async fn wait_for_any(driver: &WebDriver, selectors: &[By], timeout_secs: u64) -> Result<WebElement> {
    find_any(driver, selectors, timeout_secs).await
}
