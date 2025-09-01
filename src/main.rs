use chrono::Local;
use clap::{Arg, ArgAction, ArgGroup, ArgMatches, Command};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use anyhow::{Context, Result, anyhow};

const VERSION: &str = "2.2.0";
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
    RemoveDesktop,DebugTags,
    Status,
    ForceRefresh,
}

#[derive(Debug)]
struct Config {
    action: Action,
    channel: Option<Channel>,
    kill_pid: Option<u32>,
    launch_after: bool,
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

        Ok(Config {
            action,
            channel,
            kill_pid,
            launch_after,
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
        } else {
            Err(anyhow!("No action specified"))
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct AppConfig {
    channel: String,
    git_url: String,
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

        //处理日期格式版本 (如 4.2.2-20240901)
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

        // 处理标准语义化版本
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
        }match self.patch.cmp(&other.patch) {
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
    install_dir: PathBuf,
    ntsport_dir: PathBuf,
    program_dir: PathBuf,
    caching_dir: PathBuf,
    startup_bin: PathBuf,
    install_bin: PathBuf,
    branch_file: PathBuf,
    git_file: PathBuf,
}

impl HoloMotionInstaller {
    fn new() -> Result<Self> {
        let home_dir = dirs::home_dir()
            .ok_or_else(|| anyhow!("Could not determine home directory"))?;

        let install_dir = home_dir.join("local/bin");
        let ntsport_dir = install_dir.join("ntsports");
        let program_dir = ntsport_dir.join("HoloMotion");
        let caching_dir = home_dir.join("Documents/HoloMotion_log");

        let startup_bin = install_dir.join("HoloMotion");
        let install_bin = install_dir.join("HoloMotion");
        let branch_file = program_dir.join("branch.txt");
        let git_file = program_dir.join("git.txt");

        Ok(Self {
            install_dir,
            ntsport_dir,
            program_dir,
            caching_dir,
            startup_bin,
            install_bin,
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

    fn get_git_url(&self) -> Result<String> {
        if self.git_file.exists() {
            let content = fs::read_to_string(&self.git_file)?;
            Ok(content.trim().to_string())
        } else {
            Err(anyhow!("Git config file not found"))
        }
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

    fn ensure_correct_remote(&self) -> Result<()> {
        if !self.repos_exist() {
            return Ok(());
        }

        let expected_url = match self.get_git_url() {
            Ok(url) => url,
            Err(_) => {
                self.log("警告: git.txt 文件不存在，跳过远程仓库检查");
                return Ok(());
            }
        };

        let current_url = match self.get_current_remote_url() {
            Ok(url) => url,
            Err(_) => {
                self.log("警告: 无法获取当前远程仓库URL");
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
            self.log(&"检测到远程仓库URL不匹配".to_string());
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

            self.log("远程仓库URL已更新");
        } else {
            self.log("远程仓库URL检查通过");
        }

        Ok(())
    }

    fn fetch_remote(&self) -> Result<()> {
        if !self.repos_exist() {
            return Err(anyhow!("Repository does not exist"));
        }

        self.log("正在获取远程仓库最新信息...");

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
            self.log(&format!("Fetch 警告/错误: {}", stderr));
            let output2 = StdCommand::new("git")
                .args(&["fetch", "--all", "--tags", "--force"])
                .current_dir(&self.program_dir)
                .output()?;
            if !output2.status.success() {
                let stderr2 = String::from_utf8_lossy(&output2.stderr);
                return Err(anyhow!("Failed to fetch from remote: {}", stderr2));
            }
        }

        let output = StdCommand::new("git")
            .args(&["tag", "-l", "--sort=-version:refname"])
            .current_dir(&self.program_dir)
            .output()?;
        if output.status.success() {
            let tags = String::from_utf8(output.stdout)?;
            let tag_count = tags.lines().count();
            self.log(&format!("远程仓库信息获取完成，共 {} 个标签", tag_count));
            let latest_tags: Vec<&str> = tags.lines().take(5).collect();
            self.log(&format!("最新标签: {:?}", latest_tags));
        }

        Ok(())
    }

    fn force_refresh_tags(&self) -> Result<()> {
        self.log("强制刷新远程标签信息...");
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
                self.log("删除所有本地标签...");
                for tag in tags.lines() {
                    let _ = StdCommand::new("git")
                        .args(&["tag", "-d", tag])
                        .current_dir(&self.program_dir)
                        .output();
                }
                self.log("本地标签已清理");
            }
        }

        let output = StdCommand::new("git")
            .args(&["fetch", "origin", "--tags", "--force"])
            .current_dir(&self.program_dir)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("Failed to fetch tags: {}", stderr));
        }

        self.log("远程标签刷新完成");
        Ok(())
    }

    fn repos_exist(&self) -> bool {
        self.program_dir.exists() && self.program_dir.join(".git").exists()
    }

    fn assert_repos_exist(&self) -> Result<()> {
        if !self.repos_exist() {
            return Err(anyhow!("Application not installed. Please use --install first!"));
        }
        Ok(())
    }

    fn get_current_channel(&self) -> Result<Channel> {
        self.log("获取当前安装通道");

        if self.branch_file.exists() {
            let channel_str = fs::read_to_string(&self.branch_file)?;
            let channel = Channel::from_str(channel_str.trim())?;
            self.log(&format!("从配置文件读取通道: {}", channel.as_str()));
            return Ok(channel);
        }

        self.assert_repos_exist()?;
        self.log("配置文件不存在，根据仓库标签判断通道");

        self.ensure_correct_remote()?;
        self.fetch_remote()?;

        let output = StdCommand::new("git")
            .args(&["describe", "--tags"])
            .current_dir(&self.program_dir)
            .output()?;

        if !output.status.success() {
            return Err(anyhow!("Failed to get git describe output"));
        }

        let raw_version = String::from_utf8(output.stdout)?.trim().to_string();
        self.log(&format!("Git describe 原始输出: {}", raw_version));

        let version = self.extract_version_from_git_describe(&raw_version)?;
        self.log(&format!("提取的版本号: {}", version));

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
        self.log(&format!("当前安装通道: {}", channel.as_str()));

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

    fn get_current_version(&self, channel: &Channel) -> Result<String> {
        self.log("获取已安装版本号");
        self.assert_repos_exist()?;
        self.log(&format!("当前通道: {}", channel.as_str()));

        self.ensure_correct_remote()?;

        let output = StdCommand::new("git")
            .args(&["describe", "--tags"])
            .current_dir(&self.program_dir)
            .output()?;

        if !output.status.success() {
            return Err(anyhow!("Failed to get git describe output"));
        }

        let raw_version = String::from_utf8(output.stdout)?.trim().to_string();
        self.log(&format!("Git describe 原始输出: {}", raw_version));

        let version = self.extract_version_from_git_describe(&raw_version)?;
        self.log(&format!("处理后的版本号: {}", version));

        if Version::parse(&version).is_ok() {
            Ok(version)
        } else {
            Err(anyhow!("Version format does not match expected pattern: {}", version))
        }
    }

    fn get_latest_version(&self, channel: &Channel) -> Result<String> {
        self.log("获取最新版版本号");
        self.assert_repos_exist()?;
        self.log(&format!("当前通道: {}", channel.as_str()));

        self.ensure_correct_remote()?;
        self.fetch_remote()?;

        self.log("方法1: 使用 git ls-remote 获取远程标签");
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

        self.log("方法2: 使用本地标签列表作为备选");
        let output = StdCommand::new("git")
            .args(&["tag", "-l", "--sort=-version:refname"])
            .current_dir(&self.program_dir)
            .output()?;

        let mut versions_method2 = Vec::new();
        if output.status.success() {
            let tags_output = String::from_utf8(output.stdout)?;
            for line in tags_output.lines().take(20) {
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
            self.log(&format!("使用方法1结果，获取到 {} 个版本", versions_method1.len()));
            versions_method1
        } else {
            self.log(&format!("使用方法2结果，获取到 {} 个版本", versions_method2.len()));
            versions_method2
        };

        if versions.is_empty() {
            return Err(anyhow!("没有找到符合通道 {} 的有效版本", channel.as_str()));
        }

        versions.sort();
        let latest = versions.last().unwrap();
        self.log(&format!("找到 {} 个有效版本", versions.len()));
        self.log(&format!("远端最新版本: {}", latest.raw));
        Ok(latest.raw.clone())
    }

    fn kill_process(&self, pid: u32) -> Result<()> {
        self.log(&format!("正在关闭进程: {}", pid));

        let output = StdCommand::new("kill")
            .args(&["-9", &pid.to_string()])
            .output()?;

        if output.status.success() {
            self.log("进程已关闭");} else {
            self.log("关闭进程失败或进程不存在");
        }

        Ok(())
    }

    fn clean_installed(&self) -> Result<()> {
        self.log("开始清理");

        if self.program_dir.exists() {
            fs::remove_dir_all(&self.program_dir)?;
            self.log("已清理程序目录");
        }

        if self.startup_bin.exists() {
            fs::remove_file(&self.startup_bin)?;
            self.log("已清理启动脚本");
        }

        if self.caching_dir.exists() {
            fs::remove_dir_all(&self.caching_dir)?;
            self.log("已清理缓存数据");
        }

        self.log("清理完成");
        Ok(())
    }

    fn install(&self, channel: &Channel) -> Result<()> {
        self.log("开始安装");
        self.log(&format!("安装目录: {:?}, 通道: {}", self.ntsport_dir, channel.as_str()));

        self.clean_installed()?;

        fs::create_dir_all(&self.ntsport_dir)?;
        self.log(&format!("创建程序安装目录: {:?}", self.ntsport_dir));

        let git_url = self.get_git_url()?;

        self.log("正在下载程序");
        let output = StdCommand::new("git")
            .args(&["clone", &git_url, &self.program_dir.to_string_lossy()])
            .current_dir(&self.ntsport_dir)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("Failed to clone repository: {}", stderr));
        }

        let _ = StdCommand::new("git")
            .args(&["config", "--global", "--add", "safe.directory", &self.program_dir.to_string_lossy()])
            .output();

        self.fetch_remote()?;
        let latest_version = self.get_latest_version(channel)?;
        self.log(&format!("正在切换到版本: {}", latest_version));

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
                return Err(anyhow!("切换到最新版本失败: {}", stderr));
            }
        }

        fs::write(&self.branch_file, channel.as_str())?;
        self.log(&format!("写入配置文件: channel={}", channel.as_str()));

        self.log(&format!("安装完成! 版本: {}", latest_version));
        Ok(())
    }

    fn upgrade(&self, channel: &Channel) -> Result<()> {
        self.log("开始升级");
        self.assert_repos_exist()?;

        self.ensure_correct_remote()?;
        self.fetch_remote()?;

        let current_version = self.get_current_version(channel)?;
        let latest_version = self.get_latest_version(channel)?;

        self.log(&format!("当前版本: {}", current_version));
        self.log(&format!("最新版本: {}", latest_version));

        if current_version == latest_version {
            self.log("已经是最新版本!");
            return Ok(());
        }

        self.log("正在应用更新");
        let output = StdCommand::new("git")
            .args(&["reset", "--hard"])
            .current_dir(&self.program_dir)
            .output()?;

        if !output.status.success() {
            return Err(anyhow!("Failed to reset git state"));
        }

        self.log(&format!("正在切换到版本: {}", latest_version));
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
                return Err(anyhow!("Failed to switch to latest version: {}", stderr));
            }
        }

        fs::write(&self.branch_file, channel.as_str())?;

        self.log(&format!("升级完成! 版本: {} -> {}", current_version, latest_version));
        Ok(())
    }

    fn uninstall(&self) -> Result<()> {
        self.log("开始卸载");
        self.clean_installed()?;
        self.log("卸载完成!");
        Ok(())
    }

    fn launch(&self) -> Result<()> {
        self.log("启动程序");
        self.assert_repos_exist()?;
        let startup_script = self.program_dir.join("NT.Client.sh");

        if !startup_script.exists() {
            return Err(anyhow!("Startup script not found"));
        }

        let _child = StdCommand::new("sh")
            .arg(&startup_script)
            .spawn()?;

        self.log("程序已启动");
        Ok(())
    }

    fn create_desktop_entry(&self) -> Result<()> {
        let desktop_content = format!(
            "[Desktop Entry]\n\
Type=Application\n\
Name=HoloMotion\n\
GenericName=HoloMotion\n\
Comment=HoloMotion Application\n\
Exec={}\n\
Icon={}\n\
Terminal=false\n\
Categories=Application;Development;\n\
StartupNotify=true\n",
            self.startup_bin.display(),
            self.program_dir.join("assets/watermark_logo.png").display()
        );

        let desktop_file = Path::new("/usr/share/applications/HoloMotion.desktop");
        fs::write(desktop_file, desktop_content)?;

        let _ = StdCommand::new("chmod")
            .args(&["644", desktop_file.to_str().unwrap()])
            .output();
        self.log("桌面图标创建成功");
        Ok(())
    }

    fn remove_desktop_entry(&self) -> Result<()> {
        let desktop_file = Path::new("/usr/share/applications/HoloMotion.desktop");
        if desktop_file.exists() {
            fs::remove_file(desktop_file)?;
            self.log("桌面图标删除成功");
        } else {
            self.log("桌面图标不存在");
        }
        Ok(())
    }

    fn debug_list_tags(&self) -> Result<()> {
        self.log("=== 调试信息: 当前仓库标签 ===");

        if !self.repos_exist() {
            self.log("仓库不存在");
            return Ok(());
        }

        let output = StdCommand::new("git")
            .args(&["tag", "-l", "--sort=-version:refname"])
            .current_dir(&self.program_dir)
            .output()?;
        if output.status.success() {
            let tags = String::from_utf8(output.stdout)?;
            self.log("本地标签(按版本排序):");
            for tag in tags.lines().take(20) {
                self.log(&format!("  {}", tag));
            }}let output = StdCommand::new("git")
            .args(&["ls-remote", "--tags", "origin"])
            .current_dir(&self.program_dir)
            .output()?;

        if output.status.success() {
            let tags = String::from_utf8(output.stdout)?;
            self.log("远程标签:");
            for line in tags.lines().take(20) {
                self.log(&format!("  {}", line));
            }
        }

        self.log("=== 调试信息结束 ===");
        Ok(())
    }

    fn check_status(&self) -> Result<()> {
        self.log("=== 系统状态检查 ===");

        if self.repos_exist() {
            self.log("✓ 应用程序已安装");
            if let Ok(channel) = self.get_current_channel() {
                self.log(&format!("✓ 当前通道: {}", channel.as_str()));

                if let Ok(current_version) = self.get_current_version(&channel) {
                    self.log(&format!("✓ 当前版本: {}", current_version));
                    let _ = self.ensure_correct_remote();
                    let _ = self.fetch_remote();
                    if let Ok(latest_version) = self.get_latest_version(&channel) {
                        self.log(&format!("✓ 最新版本: {}", latest_version));
                        if current_version == latest_version {
                            self.log("✓ 已是最新版本");
                        } else {
                            self.log(&format!("⚠ 发现更新: {} -> {}", current_version, latest_version));
                        }
                    } else {
                        self.log("✗ 无法获取最新版本信息");
                    }
                } else {
                    self.log("✗ 无法获取当前版本信息");
                }
            } else {
                self.log("✗ 无法获取通道信息");
            }
        } else {
            self.log("✗ 应用程序未安装");
        }

        self.log("=== 状态检查完成 ===");
        Ok(())
    }

    fn execute_action(&self, config: &Config) -> Result<()> {
        // Handle process killing if specified
        if let Some(pid) = config.kill_pid {
            self.kill_process(pid)?;
        }

        // Determine channel
        let channel = config.channel.clone().unwrap_or_else(|| {
            self.get_current_channel().unwrap_or(Channel::Release)
        });

        match &config.action {
            Action::GetCurrentChannel => {
                let current_channel = self.get_current_channel()?;
                println!("{}", current_channel.as_str());
            }
            Action::GetCurrentVersion => {
                let version = self.get_current_version(&channel)?;
                println!("{}", version);
            }
            Action::GetLatestVersion => {
                let version = self.get_latest_version(&channel)?;
                println!("{}", version);
            }
            Action::Install => {
                self.install(&channel)?;
                if config.launch_after {
                    self.launch()?;
                }
            }
            Action::Upgrade => {
                self.upgrade(&channel)?;
                if config.launch_after {
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
        }

        Ok(())
    }
}

fn build_cli() -> Command {
    Command::new("HoloMotion Installer")
        .version(VERSION)
        .author("HoloMotion Team")
        .about("HoloMotion application installer and updater with advanced CLI")
        .help_template("\
{before-help}{name} {version}
{author-with-newline}{about-with-newline}
{usage-heading} {usage}

{all-args}

{after-help}")
        .after_help("Examples:holomotion-installer --install -b release -rInstall with release channel and launch after completion
  holomotion-installer --upgrade -k1234
      Upgrade after killing process1234
  holomotion-installer --get-latest-version
      Check latest available version")
        //短参数组
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

        // 长参数组 - 与bash脚本完全一致
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

        // 调试命令
        .arg(Arg::new("debug-tags")
            .long("debug-tags")
            .help("调试: 列出所有标签")
            .action(ArgAction::SetTrue))
        .arg(Arg::new("status")
            .long("status")
            .help("检查系统和安装状态")
            .action(ArgAction::SetTrue))
        .arg(Arg::new("force-refresh")
            .long("force-refresh")
            .help("强制刷新远程标签")
            .action(ArgAction::SetTrue))

        // 参数组定义
        .group(ArgGroup::new("action")
            .required(true)
            .args([
                "get-current-channel", "current-channel",
                "get-current-version", "current-version",
                "get-latest-version", "latest-version",
                "install", "upgrade", "uninstall", "launch-only",
                "create-desktop", "remove-desktop",
                "debug-tags", "status", "force-refresh","version", "help"]))
}

fn main() -> Result<()> {
    let matches = build_cli().get_matches();

    // Handle help first
    if matches.get_flag("help") {
        build_cli().print_help()?;
        return Ok(());
    }

    // Handle version
    if matches.get_flag("version") {
        println!("{} - {}", VERSION, PUBLISH_DATE);
        return Ok(());
    }

    let config = Config::from_matches(&matches)?;
    let installer = HoloMotionInstaller::new()?;
    installer.execute_action(&config)?;

    Ok(())
}