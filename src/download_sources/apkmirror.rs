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
                sleep(sleep_duration).await;
                match download_app(&app_id, version.as_deref(), output_path, &options).await {
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
    version: Option<&str>,
    output_path: &Path,
    options: &HashMap<&str, &str>,
) -> Result<String, String> {
    // Create a client with appropriate headers
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36")
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    // Search for the app
    let search_url = format!("https://www.apkmirror.com/?post_type=app_release&searchtype=apk&s={}", app_id);
    println!("Searching for {} on APKMirror", app_id);
    
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
    
    // Find the app link in search results
    let app_url_re = Regex::new(r#"href="(https://www\.apkmirror\.com/apk/[^"]+)"#).unwrap();
    let app_url = html.lines()
        .find(|line| line.contains("widget_appRow") && line.contains(app_id))
        .and_then(|line| {
            app_url_re.captures(line).map(|cap| cap[1].to_string())
        })
        .ok_or_else(|| format!("App {} not found on APKMirror", app_id))?;
    
    println!("Found app page: {}", app_url);
    
    // Check if we need a specific version
    let version_page_url = if let Some(version_str) = version {
        // Try to find the specific version
        let version_search_url = format!("{}/?q={}", app_url, version_str);
        println!("Searching for version {}", version_str);
        
        let version_search_response = client.get(&version_search_url)
            .send()
            .await
            .map_err(|e| format!("Failed to search for version: {}", e))?;
        
        if !version_search_response.status().is_success() {
            return Err(format!("Failed to search for version: HTTP {}", version_search_response.status()));
        }
        
        let version_search_html = version_search_response.text()
            .await
            .map_err(|e| format!("Failed to read version search response: {}", e))?;
        
        // Find the version link
        let version_url_re = Regex::new(r#"href="(https://www\.apkmirror\.com/apk/[^"]+/[^"]+/[^"]+?-release/[^"]+)"#).unwrap();
        version_search_html.lines()
            .find(|line| line.contains(version_str) && line.contains("downloadButton"))
            .and_then(|line| {
                version_url_re.captures(line).map(|cap| cap[1].to_string())
            })
            .ok_or_else(|| format!("Version {} not found for {}", version_str, app_id))?
    } else {
        // Get the app page to find the latest version
        let app_response = client.get(&app_url)
            .send()
            .await
            .map_err(|e| format!("Failed to access app page: {}", e))?;
        
        if !app_response.status().is_success() {
            return Err(format!("Failed to access app page: HTTP {}", app_response.status()));
        }
        
        let app_html = app_response.text()
            .await
            .map_err(|e| format!("Failed to read app page: {}", e))?;
        
        // Find the latest version link
        let latest_url_re = Regex::new(r#"href="(https://www\.apkmirror\.com/apk/[^"]+/[^"]+/[^"]+?-release/[^"]+)"#).unwrap();
        app_html.lines()
            .find(|line| line.contains("downloadButton"))
            .and_then(|line| {
                latest_url_re.captures(line).map(|cap| cap[1].to_string())
            })
            .ok_or_else(|| format!("Latest version link not found for {}", app_id))?
    };
    
    println!("Found version page: {}", version_page_url);
    
    // Get the version page
    let version_page_response = client.get(&version_page_url)
        .send()
        .await
        .map_err(|e| format!("Failed to access version page: {}", e))?;
    
    if !version_page_response.status().is_success() {
        return Err(format!("Failed to access version page: HTTP {}", version_page_response.status()));
    }
    
    let version_page_html = version_page_response.text()
        .await
        .map_err(|e| format!("Failed to read version page: {}", e))?;
    
    // Find the download page link
    let download_page_re = Regex::new(r#"href="(/apk/[^"]+/download)"#).unwrap();
    let download_page_path = download_page_re
        .captures(&version_page_html)
        .and_then(|cap| cap.get(1))
        .map(|m| m.as_str().to_string())
        .ok_or_else(|| format!("Download page link not found for {}", app_id))?;
    
    let download_page_url = format!("https://www.apkmirror.com{}", download_page_path);
    println!("Found download page: {}", download_page_url);
    
    // Access the download page
    let download_page_response = client.get(&download_page_url)
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
    let download_url_re = Regex::new(r#"href="([^"]+)"[^>]*>Download APK</a>#).unwrap();
    let download_path = download_url_re
        .captures(&download_page_html)
        .and_then(|cap| cap.get(1))
        .map(|m| m.as_str().to_string())
        .ok_or_else(|| format!("Final download link not found for {}", app_id))?;
    
    let download_url = format!("https://www.apkmirror.com{}", download_path);
    println!("Downloading APK from: {}", download_url);
    
    // Download the APK file
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static(
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36"
    ));
    
    let response = client.get(&download_url)
        .headers(headers)
        .send()
        .await
        .map_err(|e| format!("Failed to download APK: {}", e))?;
    
    if !response.status().is_success() {
        return Err(format!("Failed to download APK: HTTP {}", response.status()));
    }
    
    // Generate filename - either from response or use app_id with version
    let filename = response
        .headers()
        .get("content-disposition")
        .and_then(|header| {
            header.to_str().ok().and_then(|s| {
                let re = Regex::new(r#"filename=(?:"([^"]+)"|([^;]+))"#).unwrap();
                re.captures(s).map(|cap| {
                    cap.get(1).unwrap_or_else(|| cap.get(2).unwrap()).as_str().to_string()
                })
            })
        })
        .unwrap_or_else(|| {
            if let Some(v) = version {
                format!("{}-{}.apk", app_id, v)
            } else {
                format!("{}.apk", app_id)
            }
        });
    
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
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36")
        .build()
        .unwrap();
    
    for (app_id, _) in app_ids {
        println!("Listing versions for {} on APKMirror:", app_id);
        
        // Search for the app
        let search_url = format!("https://www.apkmirror.com/?post_type=app_release&searchtype=apk&s={}", app_id);
        let response = match client.get(&search_url).send().await {
            Ok(resp) => resp,
            Err(e) => {
                println!("Error searching for app {}: {}", app_id, e);
                continue;
            }
        };
        
        if !response.status().is_success() {
            println!("Failed to search for app {}: HTTP {}", app_id, response.status());
            continue;
        }
        
        let html = match response.text().await {
            Ok(html) => html,
            Err(e) => {
                println!("Error reading search response for {}: {}", app_id, e);
                continue;
            }
        };
        
        // Find the app link in search results
        let app_url_re = Regex::new(r#"href="(https://www\.apkmirror\.com/apk/[^"]+)"#).unwrap();
        let app_url = match html.lines()
            .find(|line| line.contains("widget_appRow") && line.contains(&app_id))
            .and_then(|line| {
                app_url_re.captures(line).map(|cap| cap[1].to_string())
            }) {
                Some(url) => url,
                None => {
                    println!("App {} not found on APKMirror", app_id);
                    continue;
                }
            };
        
        // Access the app page to list versions
        let app_response = match client.get(&app_url).send().await {
            Ok(resp) => resp,
            Err(e) => {
                println!("Error accessing app page for {}: {}", app_id, e);
                continue;
            }
        };
        
        if !app_response.status().is_success() {
            println!("Failed to access app page for {}: HTTP {}", app_id, app_response.status());
            continue;
        }
        
        let app_html = match app_response.text().await {
            Ok(html) => html,
            Err(e) => {
                println!("Error reading app page for {}: {}", app_id, e);
                continue;
            }
        };
        
        // Extract versions
        let version_re = Regex::new(r#"<div class="infoSlide-value"[^>]*>([^<]+)</div>"#).unwrap();
        let date_re = Regex::new(r#"<p class="datetime_utc"[^>]*>([^<]+)</p>"#).unwrap();
        
        let mut versions = Vec::new();
        
        for line in app_html.lines() {
            if let Some(version_cap) = version_re.captures(line) {
                let version = version_cap[1].trim().to_string();
                // Try to find the date in nearby lines
                if let Some(date) = date_re.captures(line).or_else(|| {
                    app_html.lines()
                        .skip_while(|l| !l.contains(&version))
                        .take(5)
                        .find_map(|l| date_re.captures(l))
                }) {
                    versions.push((version, date[1].trim().to_string()));
                } else {
                    versions.push((version, "Unknown date".to_string()));
                }
            }
        }
        
        // Print the versions
        if versions.is_empty() {
            println!("No versions found for {}", app_id);
        } else {
            for (version, date) in versions {
                println!("Version: {} ({})", version, date);
            }
        }
        
        println!(); // Add a blank line between apps
    }
}