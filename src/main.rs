use anyhow::{anyhow, Result};
use chrono::Local;
use clap::{Arg, ArgAction, ArgGroup, ArgMatches, Command};
use regex::Regex;
use std::cmp::Ordering;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

const VERSION: &str = "2.5.0";
const PUBLISH_DATE: &str = "2024-09-01";

#[derive(Debug, Clone, PartialEq)]
enum Channel {
    Master,
    Release,
}

impl Channel {
    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "master" => Ok(Channel::Master),
            "release" => Ok(Channel::Release),
            _ => Err(anyhow!("Invalid channel: {}. Available channels: master, release", s)),
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            Channel::Master => "master",
            Channel::Release => "release",
        }
    }
}

#[derive(Debug)]
enum Action {
    GetCurrentChannel,
    GetCurrentVersion,
    GetLatestVersion,
    Install,
    Upgrade,
    Uninstall,
    Launch,
    CreateDesktop,
    RemoveDesktop,
    DebugTags,
    Status,
    ForceRefresh,
    UpdateGitUrl,
}

#[derive(Debug)]
struct Config {
    action: Action,
    channel: Option<Channel>,
    kill_pid: Option<u32>,
    launch_after: bool,
    app_name: String,
    git_url: Option<String>,
}

impl Config {
    fn from_matches(matches: &ArgMatches) -> Result<Self> {
        let action = Self::determine_action(matches)?;

        let channel = matches
            .get_one::<String>("channel")
            .map(|s| Channel::from_str(s))
            .transpose()?;
        let kill_pid = matches.get_one::<u32>("kill").copied();
        let launch_after = matches.get_flag("launch");
        let app_name = if let Some(name) = matches.get_one::<String>("app-name") {
            name.to_string()
        } else if let Some(detected_name) = HoloMotionInstaller::detect_app_name_from_current_dir() {
            detected_name
        } else {
            "HoloMotion".to_string()
        };

        let git_url = matches
            .get_one::<String>("git-url")
            .or_else(|| matches.get_one::<String>("update-git-url"))
            .map(|s| s.to_string());

        Ok(Config {
            action,
            channel,
            kill_pid,
            launch_after,
            app_name,
            git_url,
        })
    }

    fn determine_action(matches: &ArgMatches) -> Result<Action> {
        if matches.get_flag("get-current-channel") || matches.get_flag("current-channel") {
            Ok(Action::GetCurrentChannel)
        } else if matches.get_flag("get-current-version") || matches.get_flag("current-version") {
            Ok(Action::GetCurrentVersion)
        } else if matches.get_flag("get-latest-version") || matches.get_flag("latest-version") {
            Ok(Action::GetLatestVersion)
        } else if matches.get_flag("install") {
            Ok(Action::Install)
        } else if matches.get_flag("upgrade") {
            Ok(Action::Upgrade)
        } else if matches.get_flag("uninstall") {
            Ok(Action::Uninstall)
        } else if matches.get_flag("launch-only") {
            Ok(Action::Launch)
        } else if matches.get_flag("create-desktop") {
            Ok(Action::CreateDesktop)
        } else if matches.get_flag("remove-desktop") {
            Ok(Action::RemoveDesktop)
        } else if matches.get_flag("debug-tags") {
            Ok(Action::DebugTags)
        } else if matches.get_flag("status") {
            Ok(Action::Status)
        } else if matches.get_flag("force-refresh") {
            Ok(Action::ForceRefresh)
        } else if matches.contains_id("update-git-url") {
            Ok(Action::UpdateGitUrl)
        } else {
            Err(anyhow!("No action specified"))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Version {
    major: u32,
    minor: u32,
    patch: u32,
    pre_release: Option<String>,
    build_metadata: Option<String>,
    raw: String,
}

impl Version {
    fn parse(version_str: &str) -> Result<Self> {
        let raw = version_str.to_string();

        let date_regex = Regex::new(r"^(\d+)\.(\d+)\.(\d+)-(\d{8})$")?;
        if let Some(captures) = date_regex.captures(version_str) {
            return Ok(Version {
                major: captures[1].parse()?,
                minor: captures[2].parse()?,
                patch: captures[3].parse()?,
                pre_release: Some(captures[4].to_string()),
                build_metadata: None,
                raw,
            });
        }

        let semver_regex = Regex::new(r"^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z\-\.]+))?(?:\+([0-9A-Za-z\-\.]+))?$")?;
        if let Some(captures) = semver_regex.captures(version_str) {
            return Ok(Version {
                major: captures[1].parse()?,
                minor: captures[2].parse()?,
                patch: captures[3].parse()?,
                pre_release: captures.get(4).map(|m| m.as_str().to_string()),
                build_metadata: captures.get(5).map(|m| m.as_str().to_string()),
                raw,
            });
        }

        Err(anyhow!("Invalid version format: {}", version_str))
    }
    fn is_release(&self) -> bool {
        self.pre_release.is_none()
    }

    fn is_date_version(&self) -> bool {
        self.pre_release
            .as_ref()
            .map(|pr| pr.len() == 8 && pr.chars().all(|c| c.is_ascii_digit()))
            .unwrap_or(false)
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.major.cmp(&other.major) {
            Ordering::Equal => {},
            other => return other,
        }
        match self.minor.cmp(&other.minor) {
            Ordering::Equal => {},
            other => return other,
        }

        match self.patch.cmp(&other.patch) {
            Ordering::Equal => {},
            other => return other,
        }

        match(&self.pre_release, &other.pre_release) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(a), Some(b)) => {
                if self.is_date_version() && other.is_date_version() {
                    a.cmp(b)
                } else {
                    a.cmp(b)
                }
            }
        }
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

struct HoloMotionInstaller {
    app_name: String,
    ntsport_dir: PathBuf,
    program_dir: PathBuf,
    caching_dir: PathBuf,
    startup_bin: PathBuf,
    installer_bin: PathBuf,
    branch_file: PathBuf,
    git_file: PathBuf,
}

impl HoloMotionInstaller {
    fn detect_app_name_from_current_dir() -> Option<String> {
        if let Ok(current_dir) = std::env::current_dir() {
            if let Some(dir_name) = current_dir.file_name() {
                if let Some(dir_str) = dir_name.to_str() {
                    if dir_str.starts_with("HoloMotion") {
                        return Some(dir_str.to_string());
                    }
                }
            }
        }
        None
    }

    fn new(app_name: Option<&str>) -> Result<Self> {
        let home_dir = dirs::home_dir()
            .ok_or_else(|| anyhow!("Could not determine home directory"))?;

        let app_name = if let Some(name) = app_name {
            name.to_string()
        } else if let Some(detected_name) = Self::detect_app_name_from_current_dir() {
            println!("🔍 自动检测到应用名称: {}", detected_name);
            detected_name
        } else {
            "HoloMotion".to_string()
        };

        let ntsport_dir = home_dir.join("local/bin/ntsports");
        let program_dir = ntsport_dir.join(&app_name);
        let caching_dir = home_dir.join("Documents/HoloMotion_log");

        let startup_bin = home_dir.join("local/bin").join(&app_name);
        let installer_bin = home_dir.join("local/bin").join(format!("{}_Update", &app_name));
        let branch_file = program_dir.join("branch.txt");
        let git_file = program_dir.join("git.txt");

        Ok(Self {
            app_name,
            ntsport_dir,
            program_dir,
            caching_dir,
            startup_bin,
            installer_bin,
            branch_file,
            git_file,
        })
    }

    fn log(&self, message: &str) {
        let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        println!("{}", message);

        if let Ok(log_dir) = self.ensure_log_dir() {
            let log_file = log_dir.join(format!("{}.log", Local::now().format("%Y%m%d")));
            if let Ok(mut file) = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_file)
            {
                let _ = writeln!(file, "{} : {}", timestamp, message);
            }
        }
    }

    fn ensure_log_dir(&self) -> Result<PathBuf> {
        let log_dir = self.caching_dir.join("log/update");
        fs::create_dir_all(&log_dir)?;
        Ok(log_dir)
    }

    fn fix_permissions(&self) -> Result<()> {
        self.log("正在修正程序执行权限...");

        let files_to_chmod = vec![
            self.program_dir.join("HoloMotion_Update_installer_new.sh"),
            self.program_dir.join("NT.Client.sh"),
            self.program_dir.join("NT.Client"),
            self.program_dir.join("NT.Config.sh"),
            self.program_dir.join("NT.Config"),
        ];

        for file_path in files_to_chmod {
            if file_path.exists() {
                let output = StdCommand::new("chmod")
                    .args(&["777", file_path.to_string_lossy().as_ref()])
                    .output()?;
                if output.status.success() {
                    self.log(&format!("chmod 777 {}", file_path.display()));
                }
            }
        }

        self.log("权限修正完成");
        Ok(())
    }

    fn is_valid_git_url(&self, url: &str) -> bool {
        let url = url.trim();
        if url.is_empty() {
            return false;
        }

        let patterns = vec![
            Regex::new(r"^https://[^/\s]+/.+$").unwrap(),
            Regex::new(r"^http://[^/\s]+/.+$").unwrap(),
            Regex::new(r"^git@[^:\s]+:.+$").unwrap(),
            Regex::new(r"^ssh://git@[^/\s]+/.+$").unwrap(),
            Regex::new(r"^file://.+$").unwrap(),
        ];

        if !patterns.iter().any(|pattern| pattern.is_match(url)) {
            return false;
        }

        if url.contains("://") {
            let parts: Vec<&str> = url.split("://").collect();
            if parts.len() != 2 {
                return false;
            }

            let protocol = parts[0];
            let rest = parts[1];

            let valid_protocols = ["http", "https", "ssh", "git", "file"];
            if !valid_protocols.contains(&protocol) {
                return false;
            }

            if protocol == "file" {
                return !rest.is_empty();
            }

            if rest.contains('/') {
                let url_parts: Vec<&str> = rest.split('/').collect();
                if url_parts.len() < 2 {
                    return false;
                }
                let domain = url_parts[0];
                return !domain.is_empty();
            }
        } else if url.starts_with("git@") {
            if !url.contains(':') {
                return false;
            }
            let parts: Vec<&str> = url.split(':').collect();
            if parts.len() < 2 {
                return false;
            }
            let host_part = parts[0];
            let path_part = parts[1];
            return host_part.starts_with("git@") && !path_part.is_empty();
        }

        true
    }

    fn test_git_connectivity(&self, git_url: &str) -> Result<bool> {
        self.log(&format!("正在测试Git仓库连通性: {}", git_url));

        let output = StdCommand::new("git")
            .args(&["ls-remote", "--heads", git_url])
            .output();

        match output {
            Ok(result) => {
                if result.status.success() {
                    self.log("✓ Git仓库连通性测试通过");
                    Ok(true)
                } else {
                    let stderr = String::from_utf8_lossy(&result.stderr);
                    self.log(&format!("⚠ Git仓库连通性测试失败: {}", stderr));
                    Ok(false)
                }
            }
            Err(e) => {
                self.log(&format!("⚠ Git连通性测试执行失败: {}", e));
                Ok(false)
            }
        }
    }

    /// **修复：优先使用git.txt的Git URL获取逻辑**
    fn get_git_url(&self, provided_git_url: Option<&str>) -> Result<String> {
        // **优先级1：git.txt文件中的配置**
        if self.git_file.exists() {
            match fs::read_to_string(&self.git_file) {
                Ok(content) => {
                    let git_url = content.trim().to_string();
                    if !git_url.is_empty() {
                        self.log(&format!("✅ 优先使用git.txt配置文件中的Git仓库地址: {}", git_url));
                        return Ok(git_url);
                    } else {
                        self.log("⚠ git.txt文件存在但内容为空，尝试使用用户提供的URL");
                    }
                }
                Err(e) => {
                    self.log(&format!("⚠ 读取git.txt文件失败: {},尝试使用用户提供的URL", e));
                }
            }
        } else {
            self.log("ℹ git.txt文件不存在，将使用用户提供的URL");
        }

        // **优先级2：用户传入的Git URL**
        if let Some(git_url) = provided_git_url {
            self.log(&format!("📥 使用用户提供的Git仓库地址: {}", git_url));

            if !self.is_valid_git_url(git_url) {
                self.log(&format!("⚠ Git URL格式检查失败，但仍将尝试使用: {}", git_url));
            } else {
                self.log("✅ Git URL格式验证通过");
            }

            if let Ok(connected) = self.test_git_connectivity(git_url) {
                if !connected {
                    self.log("⚠ Git仓库连通性测试失败，但将继续尝试");
                }}

            //只有在git.txt不存在或为空时才保存
            if !self.git_file.exists() ||
                fs::read_to_string(&self.git_file).map(|s| s.trim().is_empty()).unwrap_or(true) {
                if let Err(e) = self.save_git_url(git_url) {
                    self.log(&format!("⚠ 无法保存Git配置到文件: {}", e));
                } else {
                    self.log("💾 新的Git仓库地址已保存到配置文件");
                }
            }

            return Ok(git_url.to_string());
        }Err(anyhow!("❌ 未找到Git仓库配置。请使用 --git-url 参数指定仓库地址，或确保 git.txt 文件存在"))
    }

    fn update_git_url(&self, new_git_url: &str) -> Result<()> {
        self.log(&format!("🔄 强制更新Git仓库地址: {}", new_git_url));

        if !self.is_valid_git_url(new_git_url) {
            return Err(anyhow!("❌ 无效的Git URL格式: {}", new_git_url));
        }

        if !self.test_git_connectivity(new_git_url)? {
            return Err(anyhow!("❌ Git仓库连通性测试失败: {}", new_git_url));
        }

        self.save_git_url(new_git_url)?;

        if self.repos_exist() {
            let output = StdCommand::new("git")
                .args(&["remote", "set-url", "origin", new_git_url])
                .current_dir(&self.program_dir)
                .output()?;

            if !output.status.success() {
                return Err(anyhow!("❌ 更新远程仓库URL失败"));
            }

            self.log("🔗 Git远程仓库URL已更新");
        }

        self.log("✅ Git配置更新完成");
        Ok(())
    }

    fn save_git_url(&self, git_url: &str) -> Result<()> {
        if let Some(parent_dir) = self.git_file.parent() {
            fs::create_dir_all(parent_dir)?;
        }

        fs::write(&self.git_file, git_url)?;
        self.log(&format!("💾 Git仓库地址已保存至: {}", self.git_file.display()));
        Ok(())
    }

    fn get_current_remote_url(&self) -> Result<String> {
        if !self.repos_exist() {
            return Err(anyhow!("Repository does not exist"));
        }

        let output = StdCommand::new("git")
            .args(&["remote", "get-url", "origin"])
            .current_dir(&self.program_dir)
            .output()?;

        if !output.status.success() {
            return Err(anyhow!("Failed to get current remote URL"));
        }

        let url = String::from_utf8(output.stdout)?.trim().to_string();
        Ok(url)
    }

    fn ensure_correct_remote(&self, provided_git_url: Option<&str>) -> Result<()> {
        if !self.repos_exist() {
            return Ok(());
        }

        let expected_url = self.get_git_url(provided_git_url)?;

        let current_url = match self.get_current_remote_url() {
            Ok(url) => url,
            Err(_) => {
                self.log("⚠ 无法获取当前远程仓库URL");
                return Ok(());
            }
        };

        let normalize_url = |url: &str| -> String {
            url.trim()
                .trim_end_matches('/')
                .trim_end_matches(".git")
                .to_lowercase()
        };

        if normalize_url(&expected_url) != normalize_url(&current_url) {
            self.log(&format!("🔄 检测到远程仓库URL不匹配"));
            self.log(&format!("当前: {}", current_url));
            self.log(&format!("期望: {}", expected_url));
            self.log("正在更新远程仓库URL...");

            let output = StdCommand::new("git")
                .args(&["remote", "set-url", "origin", &expected_url])
                .current_dir(&self.program_dir)
                .output()?;

            if !output.status.success() {
                return Err(anyhow!("Failed to update remote origin URL"));
            }

            self.log("✅ 远程仓库URL已更新");
        } else {
            self.log("✅ 远程仓库URL检查通过");
        }

        Ok(())
    }

    fn fetch_remote(&self) -> Result<()> {
        if !self.repos_exist() {
            return Err(anyhow!("Repository does not exist"));
        }

        self.log("🔄 正在获取远程仓库最新信息...");

        let _ = StdCommand::new("git")
            .args(&["remote", "prune", "origin"])
            .current_dir(&self.program_dir)
            .output();

        let output = StdCommand::new("git")
            .args(&["fetch", "origin", "--tags", "--force", "--prune-tags"])
            .current_dir(&self.program_dir)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            self.log(&format!("⚠ Fetch 警告/错误: {}", stderr));
            let output2 = StdCommand::new("git")
                .args(&["fetch", "--all", "--tags", "--force"])
                .current_dir(&self.program_dir)
                .output()?;
            if !output2.status.success() {
                let stderr2 = String::from_utf8_lossy(&output2.stderr);
                return Err(anyhow!("❌ Failed to fetch from remote: {}", stderr2));
            }
        }

        let output = StdCommand::new("git")
            .args(&["tag", "-l", "--sort=-version:refname"])
            .current_dir(&self.program_dir)
            .output()?;
        if output.status.success() {
            let tags = String::from_utf8(output.stdout)?;
            let tag_count = tags.lines().count();
            self.log(&format!("✅ 远程仓库信息获取完成，共 {} 个标签", tag_count));

            let latest_tags: Vec<&str> = tags.lines().take(5).collect();
            self.log(&format!("🏷️ 最新标签: {:?}", latest_tags));
        }

        Ok(())
    }

    fn clean_git_state(&self) -> Result<()> {
        self.log("🧹 正在清理Git工作目录状态...");

        let _ = StdCommand::new("git")
            .args(&["reset", "--hard", "HEAD"])
            .current_dir(&self.program_dir)
            .output();

        let _ = StdCommand::new("git")
            .args(&["clean", "-fd"])
            .current_dir(&self.program_dir)
            .output();

        let _ = StdCommand::new("git")
            .args(&["checkout", "."])
            .current_dir(&self.program_dir)
            .output();

        self.log("✅ Git工作目录状态清理完成");
        Ok(())
    }

    fn force_refresh_tags(&self) -> Result<()> {
        self.log("🔄 强制刷新远程标签信息...");

        if !self.repos_exist() {
            return Err(anyhow!("Repository does not exist"));
        }

        let output = StdCommand::new("git")
            .args(&["tag", "-l"])
            .current_dir(&self.program_dir)
            .output()?;
        if output.status.success() {
            let tags = String::from_utf8(output.stdout)?;
            if !tags.trim().is_empty() {
                self.log("🗑️ 删除所有本地标签...");
                for tag in tags.lines() {
                    let _ = StdCommand::new("git")
                        .args(&["tag", "-d", tag])
                        .current_dir(&self.program_dir)
                        .output();
                }
                self.log("✅ 本地标签已清理");
            }
        }

        let output = StdCommand::new("git")
            .args(&["fetch", "origin", "--tags", "--force"])
            .current_dir(&self.program_dir)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("❌ Failed to fetch tags: {}", stderr));
        }

        self.log("✅ 远程标签刷新完成");
        Ok(())
    }

    fn repos_exist(&self) -> bool {
        self.program_dir.exists() && self.program_dir.join(".git").exists()
    }

    fn assert_repos_exist(&self) -> Result<()> {
        if !self.repos_exist() {
            return Err(anyhow!("❌ Application not installed. Please use --install first!"));
        }
        Ok(())
    }

    fn get_current_channel(&self, provided_git_url: Option<&str>) -> Result<Channel> {
        self.log("🔍 获取当前安装通道");

        if self.branch_file.exists() {
            let channel_str = fs::read_to_string(&self.branch_file)?;
            let channel = Channel::from_str(channel_str.trim())?;
            self.log(&format!("📁 从配置文件读取通道: {}", channel.as_str()));
            return Ok(channel);
        }

        self.assert_repos_exist()?;
        self.log("ℹ 配置文件不存在，根据仓库标签判断通道");

        self.ensure_correct_remote(provided_git_url)?;
        self.fetch_remote()?;

        let output = StdCommand::new("git")
            .args(&["describe", "--tags"])
            .current_dir(&self.program_dir)
            .output()?;

        if !output.status.success() {
            return Err(anyhow!("Failed to get git describe output"));
        }

        let raw_version = String::from_utf8(output.stdout)?.trim().to_string();
        self.log(&format!("📋 Git describe 原始输出: {}", raw_version));

        let version = self.extract_version_from_git_describe(&raw_version)?;
        self.log(&format!("🔢 提取的版本号: {}", version));

        let channel = if let Ok(parsed_version) = Version::parse(&version) {
            if parsed_version.is_date_version() {
                Channel::Release
            } else if parsed_version.is_release() {
                Channel::Master
            } else {
                Channel::Release
            }
        } else {
            Channel::Release
        };

        fs::write(&self.branch_file, channel.as_str())?;
        self.log(&format!("💾 当前安装通道: {}", channel.as_str()));

        Ok(channel)
    }

    fn extract_version_from_git_describe(&self, raw_version: &str) -> Result<String> {
        let mut version = raw_version.to_string();
        if version.starts_with("refs/tags/") {
            version = version.strip_prefix("refs/tags/").unwrap().to_string();
        }
        if version.contains('/') {
            let parts: Vec<&str> = version.split('/').collect();
            if let Some(last_part) = parts.last() {
                version = last_part.to_string();
            }
        }

        let git_suffix_regex = Regex::new(r"-\d+-g[a-f0-9]+$")?;
        version = git_suffix_regex.replace(&version, "").to_string();

        if version.starts_with('v') && version.len() > 1 {
            version = version[1..].to_string();
        }

        Ok(version)
    }

    fn get_current_version(&self, channel: &Channel, provided_git_url: Option<&str>) -> Result<String> {
        self.log("🔍 获取已安装版本号");
        self.assert_repos_exist()?;
        self.log(&format!("📍 当前通道: {}", channel.as_str()));

        self.ensure_correct_remote(provided_git_url)?;

        let output = StdCommand::new("git")
            .args(&["describe", "--tags"])
            .current_dir(&self.program_dir)
            .output()?;

        if !output.status.success() {
            return Err(anyhow!("Failed to get git describe output"));
        }

        let raw_version = String::from_utf8(output.stdout)?.trim().to_string();
        self.log(&format!("📋 Git describe 原始输出: {}", raw_version));

        let version = self.extract_version_from_git_describe(&raw_version)?;
        self.log(&format!("🔢 处理后的版本号: {}", version));

        if Version::parse(&version).is_ok() {
            Ok(version)
        } else {
            Err(anyhow!("❌ Version format does not match expected pattern: {}", version))
        }
    }

    fn get_latest_version(&self, channel: &Channel, provided_git_url: Option<&str>) -> Result<String> {
        self.log("🔍 获取最新版版本号");
        self.assert_repos_exist()?;
        self.log(&format!("📍 当前通道: {}", channel.as_str()));

        self.ensure_correct_remote(provided_git_url)?;
        self.fetch_remote()?;

        self.log("📡 方法1: 使用 git ls-remote 获取远程标签");
        let output = StdCommand::new("git")
            .args(&["ls-remote", "--tags", "--refs", "origin"])
            .current_dir(&self.program_dir)
            .output()?;

        let mut versions_method1 = Vec::new();
        if output.status.success() {
            let tags_output = String::from_utf8(output.stdout)?;
            for line in tags_output.lines() {
                if let Some(tag_part) = line.split("refs/tags/").nth(1) {
                    if let Ok(version_str) = self.extract_version_from_git_describe(tag_part) {
                        if let Ok(version) = Version::parse(&version_str) {
                            let should_include = match channel {
                                Channel::Release => version.is_release() || version.is_date_version(),
                                Channel::Master => true,
                            };
                            if should_include {
                                versions_method1.push(version);
                            }
                        }
                    }
                }
            }
        }

        self.log("💾 方法2: 使用本地标签列表作为备选");
        let output = StdCommand::new("git")
            .args(&["tag", "-l", "--sort=-version:refname"])
            .current_dir(&self.program_dir)
            .output()?;

        let mut versions_method2 = Vec::new();
        if output.status.success() {
            let tags_output = String::from_utf8(output.stdout)?;
            for line in tags_output.lines().take(100) {
                if let Ok(version_str) = self.extract_version_from_git_describe(line) {
                    if let Ok(version) = Version::parse(&version_str) {
                        let should_include = match channel {
                            Channel::Release => version.is_release() || version.is_date_version(),
                            Channel::Master => true,
                        };
                        if should_include {
                            versions_method2.push(version);
                        }
                    }
                }
            }
        }

        let mut versions = if versions_method1.len() >= versions_method2.len() {
            self.log(&format!("📊 使用方法1结果，获取到 {} 个版本", versions_method1.len()));
            versions_method1
        } else {
            self.log(&format!("📊 使用方法2结果，获取到 {} 个版本", versions_method2.len()));
            versions_method2
        };

        if versions.is_empty() {
            return Err(anyhow!("❌ 没有找到符合通道 {} 的有效版本", channel.as_str()));
        }

        versions.sort();
        let latest = versions.last().unwrap();
        self.log(&format!("📈 找到 {} 个有效版本", versions.len()));
        self.log(&format!("🎯 远端最新版本: {}", latest.raw));
        Ok(latest.raw.clone())
    }

    fn kill_process(&self, pid: u32) -> Result<()> {
        self.log(&format!("🔪 正在关闭进程: {}", pid));

        let output = StdCommand::new("kill")
            .args(&["-9", &pid.to_string()])
            .output()?;

        if output.status.success() {
            self.log("✅ 进程已关闭");
        } else {
            self.log("⚠ 关闭进程失败或进程不存在");
        }

        Ok(())
    }

    fn clean_installed(&self) -> Result<()> {
        self.log("🧹 开始清理");

        if self.program_dir.exists() {
            fs::remove_dir_all(&self.program_dir)?;
            self.log("✅ 已清理程序目录");
        }

        if self.startup_bin.exists() {
            fs::remove_file(&self.startup_bin)?;
            self.log("✅ 已清理启动脚本");
        }

        if self.installer_bin.exists() {
            fs::remove_file(&self.installer_bin)?;
            self.log("✅ 已清理安装器脚本");
        }

        if self.caching_dir.exists() {
            fs::remove_dir_all(&self.caching_dir)?;
            self.log("✅ 已清理缓存数据");
        }

        self.log("🎉 清理完成");
        Ok(())
    }

    fn install(&self, channel: &Channel, provided_git_url: Option<&str>) -> Result<()> {
        self.log("🚀 开始安装");
        self.log(&format!("📍 应用: {}, 通道: {}", self.app_name, channel.as_str()));

        let git_url = self.get_git_url(provided_git_url)?;
        self.log(&format!("🔗 使用Git仓库: {}", git_url));

        self.clean_installed()?;

        fs::create_dir_all(&self.ntsport_dir)?;
        self.log(&format!("📁 创建程序安装目录: {:?}", self.ntsport_dir));

        self.log("⬇️ 正在下载程序");
        let output = StdCommand::new("git")
            .args(&["clone", &git_url, &self.program_dir.to_string_lossy()])
            .current_dir(&self.ntsport_dir)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("❌ Failed to clone repository: {}\n错误详情: {}", git_url, stderr));
        }

        let _ = StdCommand::new("git")
            .args(&["config", "--global", "--add", "safe.directory", &self.program_dir.to_string_lossy()])
            .output();

        if provided_git_url.is_some() && !self.git_file.exists() {
            self.save_git_url(&git_url)?;
        }

        self.fetch_remote()?;
        let latest_version = self.get_latest_version(channel, provided_git_url)?;
        self.log(&format!("🔄 正在切换到版本: {}", latest_version));

        let output = StdCommand::new("git")
            .args(&["checkout", &latest_version])
            .current_dir(&self.program_dir)
            .output()?;

        if !output.status.success() {
            let output = StdCommand::new("git")
                .args(&["reset", "--hard", &latest_version])
                .current_dir(&self.program_dir)
                .output()?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(anyhow!("❌ 切换到最新版本失败: {}", stderr));
            }
        }

        self.create_symlinks()?;
        self.fix_permissions()?;

        fs::write(&self.branch_file, channel.as_str())?;
        self.log(&format!("💾 写入配置文件: channel={}", channel.as_str()));

        self.log(&format!("🎉 安装完成! 版本: {}", latest_version));
        Ok(())
    }

    fn create_symlinks(&self) -> Result<()> {
        self.log("🔗 正在创建符号链接...");

        let install_src = self.program_dir.join("HoloMotion_Update_installer_new.sh");
        let install_app = self.ntsport_dir.join("HoloMotion_Update_installer_new.sh");

        if install_src.exists() {
            fs::copy(&install_src, &install_app)?;
            self.log(&format!("📋 复制安装脚本: {}", install_app.display()));

            let _ = StdCommand::new("chmod")
                .args(&["+x", install_app.to_string_lossy().as_ref()])
                .output();

            if self.installer_bin.exists() {
                let _ = fs::remove_file(&self.installer_bin);
            }

            #[cfg(unix)]
            {
                use std::os::unix::fs::symlink;
                if let Ok(_) = symlink(&install_app, &self.installer_bin) {
                    self.log(&format!("🔗 创建安装器符号链接: {}", self.installer_bin.display()));
                }
            }
        }

        let startup_src = self.program_dir.join("NT.Client.sh");

        if startup_src.exists() {
            if self.startup_bin.exists() {
                let _ = fs::remove_file(&self.startup_bin);
            }

            #[cfg(unix)]
            {
                use std::os::unix::fs::symlink;
                if let Ok(_) = symlink(&startup_src, &self.startup_bin) {
                    self.log(&format!("🔗 创建启动脚本符号链接: {}", self.startup_bin.display()));
                }
            }

            let _ = StdCommand::new("chmod")
                .args(&["+x", startup_src.to_string_lossy().as_ref()])
                .output();
        }

        let _ = StdCommand::new("hash")
            .arg("-r")
            .output();

        self.log("✅ 符号链接创建完成");
        Ok(())
    }

    /// **修复生命周期问题的upgrade方法**
    fn upgrade(&self, channel: &Channel, provided_git_url: Option<&str>) -> Result<()> {
        self.log("⬆️ 开始升级");
        self.assert_repos_exist()?;

        self.ensure_correct_remote(provided_git_url)?;
        self.fetch_remote()?;

        let current_version = self.get_current_version(channel, provided_git_url)?;
        let latest_version = self.get_latest_version(channel, provided_git_url)?;

        self.log(&format!("📊 当前版本: {}", current_version));
        self.log(&format!("📊 最新版本: {}", latest_version));

        if current_version == latest_version {
            self.log("✅ 已经是最新版本!");
            return Ok(());
        }

        self.log("🔄 正在应用更新");
        self.clean_git_state()?;
        self.fix_permissions()?;

        self.log(&format!("🔄 正在切换到版本: {}", latest_version));
        // **修复生命周期问题：预先创建字符串变量**
        let fetch_refspec = format!("refs/tags/{}:refs/tags/{}", latest_version, latest_version);
        let tag_ref = format!("tags/{}", latest_version);

        let mut success = false;

        // **方法1: 直接checkout**
        self.log("🔄 尝试方法1: checkout");
        let output = StdCommand::new("git")
            .args(&["checkout", &latest_version])
            .current_dir(&self.program_dir)
            .output()?;

        if output.status.success() {
            success = true;
            self.log("✅ 使用checkout方式切换版本成功");
        } else {
            // **方法2: fetch特定tag然后reset**
            self.log("🔄 尝试方法2: fetch+reset");
            let output = StdCommand::new("git")
                .args(&["fetch", "origin", &fetch_refspec])
                .current_dir(&self.program_dir)
                .output()?;
            if output.status.success() {
                let reset_output = StdCommand::new("git")
                    .args(&["reset", "--hard", &latest_version])
                    .current_dir(&self.program_dir)
                    .output()?;

                if reset_output.status.success() {
                    success = true;
                    self.log("✅ 使用fetch+reset方式切换版本成功");
                }
            }
        }

        if !success {
            // **方法3: fetch all然后reset**
            self.log("🔄 尝试方法3: fetch-all+reset");
            let output = StdCommand::new("git")
                .args(&["fetch", "--all"])
                .current_dir(&self.program_dir)
                .output()?;

            if output.status.success() {
                let reset_output = StdCommand::new("git")
                    .args(&["reset", "--hard", &latest_version])
                    .current_dir(&self.program_dir)
                    .output();

                if reset_output.is_ok() && reset_output.unwrap().status.success() {
                    success = true;
                    self.log("✅ 使用fetch-all+reset方式切换版本成功");
                }
            }
        }

        if !success {
            // **方法4: 最后尝试使用tags路径**
            self.log("🔄 尝试方法4: tags路径");
            let output = StdCommand::new("git")
                .args(&["reset", "--hard", &tag_ref])
                .current_dir(&self.program_dir)
                .output();

            if output.is_ok() && output.unwrap().status.success() {
                success = true;
                self.log("✅ 使用tags方式切换版本成功");
            }
        }

        if !success {
            return Err(anyhow!("❌ 所有版本切换方式都失败了"));
        }

        self.create_symlinks()?;
        self.fix_permissions()?;
        fs::write(&self.branch_file, channel.as_str())?;

        self.log(&format!("🎉 升级完成! 版本: {} -> {}", current_version, latest_version));
        Ok(())
    }

    fn uninstall(&self) -> Result<()> {
        self.log("🗑️ 开始卸载");
        self.clean_installed()?;
        self.log("🎉 卸载完成!");
        Ok(())
    }

    fn launch(&self) -> Result<()> {
        self.log("🚀 启动程序");
        self.assert_repos_exist()?;
        let startup_script = self.program_dir.join("NT.Client.sh");

        if !startup_script.exists() {
            return Err(anyhow!("❌ Startup script not found"));
        }

        let _child = StdCommand::new("sh")
            .arg(&startup_script)
            .spawn()?;

        self.log("✅ 程序已启动");
        Ok(())
    }

    fn create_desktop_entry(&self) -> Result<()> {
        let startup_app = self.program_dir.join("NT.Client.sh");
        let startup_png = self.program_dir.join("assets/watermark_logo.png");

        let desktop_content = format!(
            "[Desktop Entry]\n\
Type=Application\n\
Name={}\n\
GenericName={}\n\
Comment={} Application\n\
Exec={}\n\
Icon={}\n\
Terminal=false\n\
Categories=X-Application;\n\
StartupNotify=true\n",
            self.app_name,
            self.app_name,
            self.app_name,
            startup_app.display(),
            startup_png.display()
        );

        let desktop_file = Path::new("/usr/share/applications").join(format!("{}.desktop", self.app_name));
        let home_dir = dirs::home_dir().ok_or_else(|| anyhow!("Cannot get home directory"))?;
        let autostart_file = home_dir
            .join(".config/autostart")
            .join(format!("{}.desktop", self.app_name));

        fs::write(&desktop_file, &desktop_content)?;
        self.log(&format!("🖥️ 创建系统桌面文件: {}", desktop_file.display()));

        if let Some(autostart_dir) = autostart_file.parent() {
            fs::create_dir_all(autostart_dir)?;
        }
        fs::write(&autostart_file, &desktop_content)?;
        self.log(&format!("🔄 创建自动启动文件: {}", autostart_file.display()));

        if let Some(desktop_dir) = dirs::desktop_dir() {
            let desktop_shortcut = desktop_dir.join(format!("{}.desktop", self.app_name));
            fs::write(&desktop_shortcut, &desktop_content)?;
            let _ = StdCommand::new("chmod")
                .args(&["+x", desktop_shortcut.to_string_lossy().as_ref()])
                .output();
            self.log(&format!("🖱️ 创建桌面快捷方式: {}", desktop_shortcut.display()));
        }

        let _ = StdCommand::new("chmod")
            .args(&["644", desktop_file.to_string_lossy().as_ref()])
            .output();

        self.log("🎉 桌面图标创建成功");
        Ok(())
    }

    fn remove_desktop_entry(&self) -> Result<()> {
        let desktop_files = vec![
            Path::new("/usr/share/applications").join(format!("{}.desktop", self.app_name)),
            dirs::home_dir()
                .unwrap()
                .join(".config/autostart")
                .join(format!("{}.desktop", self.app_name)),
        ];

        let mut removed_count = 0;
        for file in &desktop_files {
            if file.exists() {
                if fs::remove_file(file).is_ok() {
                    removed_count += 1;
                    self.log(&format!("🗑️ 删除文件: {}", file.display()));
                }
            }
        }

        if let Some(desktop_dir) = dirs::desktop_dir() {
            let desktop_shortcut = desktop_dir.join(format!("{}.desktop", self.app_name));
            if desktop_shortcut.exists() {
                if fs::remove_file(&desktop_shortcut).is_ok() {
                    removed_count += 1;
                    self.log(&format!("🗑️ 删除桌面快捷方式: {}", desktop_shortcut.display()));
                }
            }
        }

        self.log(&format!("🎉 桌面图标删除完成，共删除 {} 个文件", removed_count));
        Ok(())
    }

    fn debug_list_tags(&self) -> Result<()> {
        self.log("🐛 === 调试信息: 当前仓库标签 ===");
        self.log(&format!("📱 应用名称: {}", self.app_name));
        self.log(&format!("📁 程序目录: {:?}", self.program_dir));
        self.log(&format!("🔗 启动脚本: {:?} (存在: {})", self.startup_bin, self.startup_bin.exists()));
        self.log(&format!("⚙️ 安装器脚本: {:?} (存在: {})", self.installer_bin, self.installer_bin.exists()));

        if !self.repos_exist() {
            self.log("❌ 仓库不存在");
            return Ok(());
        }

        if let Ok(git_url) = self.get_git_url(None) {
            self.log(&format!("🔗 当前Git仓库: {}", git_url));
        }

        let output = StdCommand::new("git")
            .args(&["tag", "-l", "--sort=-version:refname"])
            .current_dir(&self.program_dir)
            .output()?;
        if output.status.success() {
            let tags = String::from_utf8(output.stdout)?;
            self.log("🏷️ 本地标签(按版本排序):");
            for tag in tags.lines().take(20) {
                self.log(&format!("  📍 {}", tag));
            }
        }

        let output = StdCommand::new("git")
            .args(&["ls-remote", "--tags", "origin"])
            .current_dir(&self.program_dir)
            .output()?;
        if output.status.success() {
            let tags = String::from_utf8(output.stdout)?;
            self.log("🌐 远程标签:");
            for line in tags.lines().take(20) {
                self.log(&format!("  📡 {}", line));
            }
        }

        self.log("🐛 === 调试信息结束 ===");
        Ok(())
    }

    fn check_status(&self) -> Result<()> {
        self.log("📊 === 系统状态检查 ===");
        self.log(&format!("📱 应用名称: {}", self.app_name));
        self.log(&format!("📁 程序目录: {:?}", self.program_dir));
        self.log(&format!("🔗 启动脚本: {:?} (存在: {})", self.startup_bin, self.startup_bin.exists()));
        self.log(&format!("⚙️ 安装器脚本: {:?} (存在: {})", self.installer_bin, self.installer_bin.exists()));

        if let Ok(git_url) = self.get_git_url(None) {
            self.log(&format!("✅ Git仓库配置: {}", git_url));
        } else {
            self.log("❌ Git仓库配置未找到");
        }

        if self.repos_exist() {
            self.log("✅ 应用程序已安装");

            if let Ok(channel) = self.get_current_channel(None) {
                self.log(&format!("📍 当前通道: {}", channel.as_str()));

                if let Ok(current_version) = self.get_current_version(&channel, None) {
                    self.log(&format!("🔢 当前版本: {}", current_version));

                    let _ = self.ensure_correct_remote(None);
                    let _ = self.fetch_remote();

                    if let Ok(latest_version) = self.get_latest_version(&channel, None) {
                        self.log(&format!("🎯 最新版本: {}", latest_version));

                        if current_version == latest_version {
                            self.log("✅ 已是最新版本");
                        } else {
                            self.log(&format!("⚠️ 发现更新: {} -> {}", current_version, latest_version));
                        }
                    } else {
                        self.log("❌ 无法获取最新版本信息");
                    }
                } else {
                    self.log("❌ 无法获取当前版本信息");
                }
            } else {
                self.log("❌ 无法获取通道信息");
            }
        } else {
            self.log("❌ 应用程序未安装");
        }

        self.log("📊 === 状态检查完成 ===");
        Ok(())
    }

    fn execute_action(&self, config: &Config) -> Result<()> {
        self.log(&format!("🎯 执行操作: {:?}, 应用: {}", config.action, config.app_name));

        if let Some(pid) = config.kill_pid {
            self.kill_process(pid)?;
        }

        let channel = config.channel.clone().unwrap_or_else(|| {
            self.get_current_channel(config.git_url.as_deref()).unwrap_or(Channel::Release)
        });

        match &config.action {
            Action::GetCurrentChannel => {
                let current_channel = self.get_current_channel(config.git_url.as_deref())?;
                println!("{}", current_channel.as_str());
            }
            Action::GetCurrentVersion => {
                let version = self.get_current_version(&channel, config.git_url.as_deref())?;
                println!("{}", version);
            }
            Action::GetLatestVersion => {
                let version = self.get_latest_version(&channel, config.git_url.as_deref())?;
                println!("{}", version);
            }
            Action::Install => {
                self.install(&channel, config.git_url.as_deref())?;
                if config.launch_after {
                    self.launch()?;
                }
            }
            Action::Upgrade => {
                self.upgrade(&channel, config.git_url.as_deref())?;if config.launch_after {
                    self.launch()?;
                }
            }
            Action::Uninstall => {
                self.uninstall()?;
            }
            Action::Launch => {
                self.launch()?;
            }
            Action::CreateDesktop => {
                self.create_desktop_entry()?;
            }
            Action::RemoveDesktop => {
                self.remove_desktop_entry()?;
            }
            Action::DebugTags => {
                self.debug_list_tags()?;
            }
            Action::Status => {
                self.check_status()?;
            }
            Action::ForceRefresh => {
                self.force_refresh_tags()?;
            }
            Action::UpdateGitUrl => {
                if let Some(git_url) = &config.git_url {
                    self.update_git_url(git_url)?;
                } else {
                    return Err(anyhow!("❌ 更新Git URL时必须提供--update-git-url参数"));
                }
            }
        }

        Ok(())
    }
}

fn build_cli() -> Command {
    Command::new("HoloMotion Installer")
        .version(VERSION)
        .author("HoloMotion Team")
        .about("HoloMotion application installer and updater with intelligent Git URL management")
        .help_template("\
{before-help}{name} {version}
{author-with-newline}{about-with-newline}
{usage-heading} {usage}

{all-args}

{after-help}")
        .after_help("Examples:
  holomotion-installer --install -b release --git-url https://cnb.cool/nts2025/repo使用指定Git仓库安装
  holomotion-installer --upgrade --name HoloMotion_Test
      优先使用git.txt配置进行升级
  holomotion-installer --update-git-url https://new-repo.com/path --name HoloMotion_Test
      强制更新Git仓库地址holomotion-installer --status
      检查状态（自动检测应用名称）")
        .arg(Arg::new("channel")
            .short('b')
            .value_name("CHANNEL")
            .help("指定通道: master, release(默认)")
            .value_parser(["master", "release"])
            .num_args(1))
        .arg(Arg::new("kill")
            .short('k')
            .value_name("PID")
            .help("指定执行前需杀死的进程 ID")
            .value_parser(clap::value_parser!(u32))
            .num_args(1))
        .arg(Arg::new("launch")
            .short('r')
            .help("在安装或升级完成后是否启动客户端")
            .action(ArgAction::SetTrue))
        .arg(Arg::new("app-name")
            .short('n')
            .long("name")
            .value_name("APP_NAME")
            .help("指定应用名称 (默认: HoloMotion, 可选: HoloMotion_Test,支持自动检测)")
            .num_args(1))
        .arg(Arg::new("git-url")
            .short('g')
            .long("git-url")
            .value_name("GIT_URL")
            .help("指定Git仓库地址 (仅在git.txt不存在时保存)")
            .num_args(1))

        .arg(Arg::new("get-current-channel")
            .long("get-current-channel")
            .help("获取当前安装的通道")
            .action(ArgAction::SetTrue))
        .arg(Arg::new("current-channel")
            .long("current-channel")
            .help("获取当前安装的通道")
            .action(ArgAction::SetTrue))
        .arg(Arg::new("get-current-version")
            .long("get-current-version")
            .help("获取当前安装的版本号")
            .action(ArgAction::SetTrue))
        .arg(Arg::new("current-version")
            .long("current-version")
            .help("获取当前安装的版本号")
            .action(ArgAction::SetTrue))
        .arg(Arg::new("get-latest-version")
            .long("get-latest-version")
            .help("获取最新的版本号")
            .action(ArgAction::SetTrue))
        .arg(Arg::new("latest-version")
            .long("latest-version")
            .help("获取最新的版本号")
            .action(ArgAction::SetTrue))
        .arg(Arg::new("create-desktop")
            .long("create-desktop")
            .help("创建快捷启动图标(sudo -E)")
            .action(ArgAction::SetTrue))
        .arg(Arg::new("remove-desktop")
            .long("remove-desktop")
            .help("删除快捷启动图标(sudo -E)")
            .action(ArgAction::SetTrue))
        .arg(Arg::new("install")
            .long("install")
            .help("安装(安装最新版并保留缓存数据)")
            .action(ArgAction::SetTrue))
        .arg(Arg::new("uninstall")
            .long("uninstall")
            .help("卸载(不保留缓存数据)")
            .action(ArgAction::SetTrue))
        .arg(Arg::new("upgrade")
            .long("upgrade")
            .help("升级")
            .action(ArgAction::SetTrue))
        .arg(Arg::new("version")
            .long("version")
            .short('v')
            .help("安装脚本版本信息")
            .action(ArgAction::SetTrue))
        .arg(Arg::new("launch-only")
            .long("launch")
            .help("启动客户端")
            .action(ArgAction::SetTrue))
        .arg(Arg::new("help")
            .long("help")
            .short('h')
            .help("帮助信息")
            .action(ArgAction::SetTrue))

        .arg(Arg::new("debug-tags")
            .long("debug-tags")
            .help("调试: 列出所有标签和路径信息")
            .action(ArgAction::SetTrue))
        .arg(Arg::new("status")
            .long("status")
            .help("检查系统和安装状态")
            .action(ArgAction::SetTrue))
        .arg(Arg::new("force-refresh")
            .long("force-refresh")
            .help("强制刷新远程标签")
            .action(ArgAction::SetTrue))
        .arg(Arg::new("update-git-url")
            .long("update-git-url")
            .value_name("GIT_URL")
            .help("强制更新Git仓库地址并保存到git.txt")
            .num_args(1))

        .group(ArgGroup::new("action")
            .required(true)
            .args([
                "get-current-channel", "current-channel",
                "get-current-version", "current-version",
                "get-latest-version", "latest-version",
                "install", "upgrade", "uninstall", "launch-only",
                "create-desktop", "remove-desktop",
                "debug-tags", "status", "force-refresh",
                "update-git-url", "version", "help"]))
}

fn main() -> Result<()> {
    let matches = build_cli().get_matches();

    if matches.get_flag("help") {
        build_cli().print_help()?;
        return Ok(());
    }

    if matches.get_flag("version") {
        println!("{} - {}", VERSION, PUBLISH_DATE);
        return Ok(());
    }

    let config = Config::from_matches(&matches)?;
    let installer = HoloMotionInstaller::new(Some(&config.app_name))?;
    installer.execute_action(&config)?;

    Ok(())
}