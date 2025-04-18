use futures::StreamExt;
use log::debug;
use regex::Regex;
use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use std::time::Duration;
use tokio::time::sleep;

pub async fn download_apps(
    app_ids: Vec<(String, Option<String>)>,
    parallel: usize,
    sleep_duration: u64,
    output_path: &Path,
    options: HashMap<&str, &str>,
) {
    let sleep_duration = Duration::from_millis(sleep_duration);
    let mut buffered = futures::stream::iter(app_ids)
        .map(|(app_id, version)| {
            async move {
                if !version.is_none() {
                    println!("Warning: APKCombo does not support downloading specific versions. Will download the latest version for {}", app_id);
                }
                sleep(sleep_duration).await;
                match download_app(&app_id, output_path, &options).await {
                    Ok(filename) => {
                        println!("Successfully downloaded {} as {}", app_id, filename);
                    }
                    Err(e) => {
                        println!("Error downloading {}: {}", app_id, e);
                    }
                }
            }
        })
        .buffer_unordered(parallel);

    while buffered.next().await.is_some() {}
}

async fn download_app(
    app_id: &str,
    output_path: &Path,
    options: &HashMap<&str, &str>,
) -> Result<String, String> {
    // Create a client with appropriate headers
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36")
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    // First search for the app
    let search_url = format!("https://apkcombo.com/search/{}/", app_id);
    println!("Searching for {} on APKCombo", app_id);
    
    let response = client.get(&search_url)
        .send()
        .await
        .map_err(|e| format!("Failed to search for app: {}", e))?;
    
    if !response.status().is_success() {
        return Err(format!("Failed to search for app: HTTP {}", response.status()));
    }
    
    let html = response.text()
        .await
        .map_err(|e| format!("Failed to read search response: {}", e))?;
    
    // Find the app page URL in search results
    let app_url_re = Regex::new(r#"href="(/[^/]+/[^/]+/[^"]+)"#).unwrap();
    let app_url = html.lines()
        .filter(|line| line.contains(app_id))
        .find_map(|line| {
            app_url_re.captures(line).map(|cap| cap[1].to_string())
        })
        .ok_or_else(|| format!("App {} not found on APKCombo", app_id))?;
    
    let full_app_url = format!("https://apkcombo.com{}", app_url);
    println!("Found app page: {}", full_app_url);
    
    // Fetch the app page to get the download URL
    let app_response = client.get(&full_app_url)
        .send()
        .await
        .map_err(|e| format!("Failed to access app page: {}", e))?;
    
    if !app_response.status().is_success() {
        return Err(format!("Failed to access app page: HTTP {}", app_response.status()));
    }
    
    let app_html = app_response.text()
        .await
        .map_err(|e| format!("Failed to read app page: {}", e))?;
    
    // Extract download link from the page
    let download_url_re = Regex::new(r#"downloadButton"\s+href="([^"]+)"#).unwrap();
    let download_url = download_url_re
        .captures(&app_html)
        .and_then(|cap| cap.get(1))
        .map(|m| m.as_str().to_string())
        .ok_or_else(|| format!("Download link not found for {}", app_id))?;
    
    let full_download_url = if download_url.starts_with("http") {
        download_url
    } else {
        format!("https://apkcombo.com{}", download_url)
    };
    println!("Found download URL: {}", full_download_url);

    // Access the download page to get the actual file
    let download_page_response = client.get(&full_download_url)
        .send()
        .await
        .map_err(|e| format!("Failed to access download page: {}", e))?;
    
    if !download_page_response.status().is_success() {
        return Err(format!("Failed to access download page: HTTP {}", download_page_response.status()));
    }
    
    let download_page_html = download_page_response.text()
        .await
        .map_err(|e| format!("Failed to read download page: {}", e))?;
    
    // Find the final download link
    let final_url_re = Regex::new(r#"href="(https://[^"]+\.apk[^"]*)"#).unwrap();
    let final_download_url = final_url_re
        .captures(&download_page_html)
        .and_then(|cap| cap.get(1))
        .map(|m| m.as_str().to_string())
        .ok_or_else(|| format!("Final APK download link not found for {}", app_id))?;
    
    println!("Downloading APK from: {}", final_download_url);
    
    // Download the APK file
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static(
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36"
    ));
    
    let response = client.get(&final_download_url)
        .headers(headers)
        .send()
        .await
        .map_err(|e| format!("Failed to download APK: {}", e))?;
    
    if !response.status().is_success() {
        return Err(format!("Failed to download APK: HTTP {}", response.status()));
    }
    
    // Generate filename from the response
    let filename = response
        .headers()
        .get("content-disposition")
        .and_then(|header| {
            header.to_str().ok().and_then(|s| {
                let re = Regex::new(r#"filename=(?:"([^"]+)"|([^;]+))"#).unwrap();
                re.captures(s).map(|cap| {
                    cap.get(1).unwrap_or_else(|| cap.get(2).unwrap()).as_str()
                })
            })
        })
        .unwrap_or_else(|| format!("{}.apk", app_id).as_str())
        .to_string();
    
    let output_file_path = output_path.join(&filename);
    
    // Save the APK file
    let apk_data = response.bytes()
        .await
        .map_err(|e| format!("Failed to read APK data: {}", e))?;
    
    let mut file = File::create(&output_file_path)
        .map_err(|e| format!("Failed to create output file: {}", e))?;
    
    file.write_all(&apk_data)
        .map_err(|e| format!("Failed to write APK data to file: {}", e))?;
    
    Ok(filename)
}

pub async fn list_versions(
    app_ids: Vec<(String, Option<String>)>,
    options: HashMap<&str, &str>,
) {
    println!("APKCombo does not support listing versions at this time.");
    println!("Only the latest version of each app is available for download.");
}