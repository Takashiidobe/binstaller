#![feature(string_remove_matches)]

use std::{
    fs::{self, File},
    io::{Cursor, Write},
    path::{Path, PathBuf},
};

use anyhow::Result;
use clap::Parser;
use directories::BaseDirs;
use reqwest::header::USER_AGENT;
use tempdir::TempDir;

mod schema;
use miniserde::json;

mod releases;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long)]
    install: String,
}

#[derive(Clone, Debug)]
struct AssetInfo {
    download_url: String,
    file_name: String,
    content_type: String,
    matches: u8,
}

impl AssetInfo {
    fn new(download_url: String, file_name: String, content_type: String) -> Self {
        let mut matches = 0;
        if file_name.contains("linux") {
            matches += 1;
        }
        if file_name.contains("x86_64") {
            matches += 1;
        }
        if file_name.contains("gnu") || file_name.contains("musl") {
            matches += 1;
        }

        Self {
            download_url,
            file_name,
            content_type,
            matches,
        }
    }
}

impl PartialEq for AssetInfo {
    fn eq(&self, other: &Self) -> bool {
        self.matches.eq(&other.matches)
    }
}

impl Eq for AssetInfo {
    fn assert_receiver_is_total_eq(&self) {}
}

impl PartialOrd for AssetInfo {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.matches.partial_cmp(&other.matches)
    }
}

impl Ord for AssetInfo {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.matches.cmp(&other.matches)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let Args { install } = Args::parse();

    let base_dirs = BaseDirs::new().expect("Couldn't find the base directory to download to");
    let exe_dir = base_dirs
        .executable_dir()
        .expect("Couldn't find executable directory");

    let url = format!("https://api.github.com/search/repositories?q={}", install);

    let client = reqwest::Client::new();

    let body = client
        .get(url)
        .header(USER_AGENT, "takashiidobe")
        .send()
        .await?
        .text()
        .await?;

    let parsed_body: schema::Root = json::from_str(&body)?;

    let mut release_url: String = parsed_body
        .items
        .into_iter()
        .take(1)
        .map(|body| body.releases_url)
        .collect::<Vec<_>>()
        .first()
        .unwrap()
        .to_owned();

    release_url.remove_matches("{/id}");

    let releases = client
        .get(release_url)
        .header(USER_AGENT, "takashiidobe")
        .send()
        .await?
        .text()
        .await?;

    let parsed_releases: releases::Root = json::from_str(&releases)?;

    let assets = parsed_releases[0].assets.clone();

    let mut asset_info: Vec<_> = assets
        .into_iter()
        .map(|a| AssetInfo::new(a.browser_download_url, a.name, a.content_type))
        .filter(|a| a.matches > 0)
        .collect();

    asset_info.sort();
    asset_info.reverse();

    let asset_to_dl = asset_info
        .first()
        .expect("There were no matching binaries to download");

    let target = asset_to_dl.download_url.clone();
    let response = reqwest::get(target).await?;

    let content = response.bytes().await?;
    let target_dir = PathBuf::from(exe_dir);

    let tmp_dir = TempDir::new("binstaller-dir")?;
    let tmp_dir_path = tmp_dir.path();

    use std::os::unix::fs::PermissionsExt;

    match asset_to_dl.content_type.as_str() {
        "application/zip" | "application/gzip" => {
            zip_extract::extract(Cursor::new(content), tmp_dir_path, true)?;
            let bin_path = tmp_dir_path.join(install.clone());
            let local_path = target_dir.join(install.clone());
            fs::rename(bin_path, local_path.clone())?;
            let mut perms = fs::metadata(local_path)?.permissions();
            perms.set_mode(0o755);
        }
        _ => unreachable!(),
    }

    // check the content_type, if application/zip, then unzip, if application/gzip, untar, and then
    // move it to the right location
    // and chmod +x the binary
    tmp_dir.close()?;

    Ok(())
}
