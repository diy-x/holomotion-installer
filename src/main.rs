use chrono::Local;
use clap::{Arg, Command};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

const VERSION: &str = "2.1.1";
const PUBLISH_DATE: &str = "2024-09-01";

#[derive(Debug, Clone, PartialEq)]
enum Channel {
    Master,
    Release,
}

impl Channel {
    fn from_str(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "master" => Ok(Channel::Master),
            "release" => Ok(Channel::Release),
            _ => Err(format!("Invalid channel: {}. Available channels: master, release", s)),
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            Channel::Master => "master",
            Channel::Release => "release",
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Config {
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
    fn parse(version_str: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let raw = version_str.to_string();

        //处理日期格式版本 (如4.2.2-20240901)
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

        Err(format!("Invalid version format: {}", version_str).into())
    }

    fn is_release(&self) -> bool {
        self.pre_release.is_none()
    }fn is_date_version(&self) -> bool {
        self.pre_release.as_ref()
            .map(|pr| pr.len() == 8&& pr.chars().all(|c| c.is_ascii_digit()))
            .unwrap_or(false)
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        // 首先比较主版本号
        match self.major.cmp(&other.major) {
            Ordering::Equal => {},
            other => return other,
        }

        // 然后比较次版本号
        match self.minor.cmp(&other.minor) {
            Ordering::Equal => {},
            other => return other,
        }// 比较修订版本号
        match self.patch.cmp(&other.patch) {
            Ordering::Equal => {},
            other => return other,
        }// 处理预发布版本的比较
        match(&self.pre_release, &other.pre_release) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater, // 正式版本 > 预发布版本
            (Some(_), None) => Ordering::Less, // 预发布版本 < 正式版本
            (Some(a), Some(b)) => {
                // 如果都是日期格式，按数字比较
                if self.is_date_version() && other.is_date_version() {
                    a.cmp(b)
                } else {
                    // 否则按字符串比较
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
    version_regex: Regex,
    version_regex_release: Regex,
    pid_regex: Regex,
}

impl HoloMotionInstaller {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let home_dir = dirs::home_dir()
            .ok_or("Could not determine home directory")?;

        let install_dir = home_dir.join("local/bin");
        let ntsport_dir = install_dir.join("ntsports");
        let program_dir = ntsport_dir.join("HoloMotion");
        let caching_dir = home_dir.join("Documents/HoloMotion_log");

        let startup_bin = install_dir.join("HoloMotion");
        let install_bin = install_dir.join("HoloMotion");
        let branch_file = program_dir.join("branch.txt");
        let git_file = program_dir.join("git.txt");

        let version_regex = Regex::new(r"^[0-9]+\.[0-9]+\.[0-9]+$")?;
        let version_regex_release = Regex::new(r"^[0-9]+\.[0-9]+\.[0-9]+-[0-9]{8}$")?;
        let pid_regex = Regex::new(r"^[0-9]+$")?;

        Ok(Self {
            install_dir,
            ntsport_dir,
            program_dir,
            caching_dir,
            startup_bin,
            install_bin,
            branch_file,
            git_file,
            version_regex,
            version_regex_release,
            pid_regex,
        })
    }

    fn log(&self, message: &str) {
        let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        println!("{}", message);

        // Write to log file
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

    fn ensure_log_dir(&self) -> Result<PathBuf, io::Error> {
        let log_dir = self.caching_dir.join("log/update");
        fs::create_dir_all(&log_dir)?;
        Ok(log_dir)
    }

    fn get_git_url(&self) -> Result<String, Box<dyn std::error::Error>> {
        if self.git_file.exists() {
            let content = fs::read_to_string(&self.git_file)?;
            Ok(content.trim().to_string())
        } else {
            Err("Git config file not found".into())
        }
    }

    fn get_current_remote_url(&self) -> Result<String, Box<dyn std::error::Error>> {
        if !self.repos_exist() {
            return Err("Repository does not exist".into());
        }

        let output = StdCommand::new("git")
            .args(&["remote", "get-url", "origin"])
            .current_dir(&self.program_dir)
            .output()?;

        if !output.status.success() {
            return Err("Failed to get current remote URL".into());
        }

        let url = String::from_utf8(output.stdout)?.trim().to_string();
        Ok(url)
    }

    /// Check and update remote origin if git.txt differs from current remote
    fn ensure_correct_remote(&self) -> Result<(), Box<dyn std::error::Error>> {
        if !self.repos_exist() {
            return Ok(()); // No repo to check
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

        // Normalize URLs for comparison (remove trailing slashes, .git suffixes, etc.)
        let normalize_url = |url: &str| -> String {
            url.trim()
                .trim_end_matches('/')
                .trim_end_matches(".git")
                .to_lowercase()
        };

        if normalize_url(&expected_url) != normalize_url(&current_url) {
            self.log(&format!("检测到远程仓库URL不匹配"));
            self.log(&format!("当前: {}", current_url));
            self.log(&format!("期望: {}", expected_url));
            self.log("正在更新远程仓库URL...");

            // Update remote origin
            let output = StdCommand::new("git")
                .args(&["remote", "set-url", "origin", &expected_url])
                .current_dir(&self.program_dir)
                .output()?;

            if !output.status.success() {
                return Err("Failed to update remote origin URL".into());
            }

            self.log("远程仓库URL已更新");
        } else {
            self.log("远程仓库URL检查通过");
        }

        Ok(())
    }

    /// Fetch latest changes from remote repository
    fn fetch_remote(&self) -> Result<(), Box<dyn std::error::Error>> {
        if !self.repos_exist() {
            return Err("Repository does not exist".into());
        }

        self.log("正在获取远程仓库最新信息...");

        //先清理可能的缓存问题
        let _ = StdCommand::new("git")
            .args(&["remote", "prune", "origin"])
            .current_dir(&self.program_dir)
            .output();

        // 获取所有分支和标签，强制更新
        let output = StdCommand::new("git")
            .args(&["fetch", "origin", "--tags", "--force", "--prune-tags"])
            .current_dir(&self.program_dir)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            self.log(&format!("Fetch 警告/错误: {}", stderr));
            //尝试另一种方式
            let output2 = StdCommand::new("git")
                .args(&["fetch", "--all", "--tags", "--force"])
                .current_dir(&self.program_dir)
                .output()?;
            if !output2.status.success() {
                let stderr2 = String::from_utf8_lossy(&output2.stderr);
                return Err(format!("Failed to fetch from remote: {}", stderr2).into());
            }
        }

        // 验证是否成功获取了标签
        let output = StdCommand::new("git")
            .args(&["tag", "-l", "--sort=-version:refname"])
            .current_dir(&self.program_dir)
            .output()?;
        if output.status.success() {
            let tags = String::from_utf8(output.stdout)?;
            let tag_count = tags.lines().count();
            self.log(&format!("远程仓库信息获取完成，共{} 个标签", tag_count));

            // 显示最新的几个标签用于调试
            let latest_tags: Vec<&str> = tags.lines().take(5).collect();
            self.log(&format!("最新标签: {:?}", latest_tags));
        }

        Ok(())
    }

    /// 强制刷新远程标签信息
    fn force_refresh_tags(&self) -> Result<(), Box<dyn std::error::Error>> {
        self.log("强制刷新远程标签信息...");
        if !self.repos_exist() {
            return Err("Repository does not exist".into());
        }

        // 删除所有本地标签
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

        // 重新获取所有标签
        let output = StdCommand::new("git")
            .args(&["fetch", "origin", "--tags", "--force"])
            .current_dir(&self.program_dir)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("Failed to fetch tags: {}", stderr).into());
        }

        self.log("远程标签刷新完成");
        Ok(())
    }

    fn repos_exist(&self) -> bool {
        self.program_dir.exists() && self.program_dir.join(".git").exists()
    }

    fn assert_repos_exist(&self) -> Result<(), Box<dyn std::error::Error>> {
        if !self.repos_exist() {
            return Err("Application not installed. Please use --install first!".into());
        }
        Ok(())
    }

    fn get_current_channel(&self) -> Result<Channel, Box<dyn std::error::Error>> {
        self.log("获取当前安装通道");

        if self.branch_file.exists() {
            let channel_str = fs::read_to_string(&self.branch_file)?;
            let channel = Channel::from_str(channel_str.trim())?;
            self.log(&format!("从配置文件读取通道: {}", channel.as_str()));
            return Ok(channel);
        }

        self.assert_repos_exist()?;
        self.log("配置文件不存在，根据仓库标签判断通道");

        // Ensure we have latest remote info before determining channel
        self.ensure_correct_remote()?;
        self.fetch_remote()?;

        let output = StdCommand::new("git")
            .args(&["describe", "--tags"])
            .current_dir(&self.program_dir)
            .output()?;

        if !output.status.success() {
            return Err("Failed to get git describe output".into());
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
                Channel::Release // 默认
            }
        } else {
            Channel::Release // 默认
        };

        //写入配置文件
        fs::write(&self.branch_file, channel.as_str())?;
        self.log(&format!("当前安装通道: {}", channel.as_str()));

        Ok(channel)
    }

    fn extract_version_from_git_describe(&self, raw_version: &str) -> Result<String, Box<dyn std::error::Error>> {
        let mut version = raw_version.to_string();
        // 移除 refs/tags/ 前缀
        if version.starts_with("refs/tags/") {
            version = version.strip_prefix("refs/tags/").unwrap().to_string();
        }

        // 处理分支格式 (如 release/4.2.2)
        if version.contains('/') {
            let parts: Vec<&str> = version.split('/').collect();
            if let Some(last_part) = parts.last() {
                version = last_part.to_string();
            }
        }

        // 移除 git describe 添加的后缀 (如 -5-g1a2b3c4)
        let git_suffix_regex = Regex::new(r"-\d+-g[a-f0-9]+$")?;
        version = git_suffix_regex.replace(&version, "").to_string();

        // 移除前导的 'v' (如 v1.2.3)
        if version.starts_with('v') && version.len() > 1 {
            version = version[1..].to_string();
        }

        Ok(version)
    }

    fn get_current_version(&self, channel: &Channel) -> Result<String, Box<dyn std::error::Error>> {
        self.log("获取已安装版本号");
        self.assert_repos_exist()?;
        self.log(&format!("当前通道: {}", channel.as_str()));

        // Ensure remote is correct and fetch latest info
        self.ensure_correct_remote()?;

        let output = StdCommand::new("git")
            .args(&["describe", "--tags"])
            .current_dir(&self.program_dir)
            .output()?;

        if !output.status.success() {
            return Err("Failed to get git describe output".into());
        }

        let raw_version = String::from_utf8(output.stdout)?.trim().to_string();
        self.log(&format!("Git describe 原始输出: {}", raw_version));

        let version = self.extract_version_from_git_describe(&raw_version)?;
        self.log(&format!("处理后的版本号: {}", version));

        // 验证版本格式
        if Version::parse(&version).is_ok() {
            Ok(version)
        } else {
            Err(format!("Version format does not match expected pattern: {}", version).into())
        }
    }

    fn get_latest_version(&self, channel: &Channel) -> Result<String, Box<dyn std::error::Error>> {
        self.log("获取最新版版本号");
        self.assert_repos_exist()?;
        self.log(&format!("当前通道: {}", channel.as_str()));

        //确保远程仓库正确并获取最新信息
        self.ensure_correct_remote()?;
        self.fetch_remote()?;

        // 方法1: 使用 git ls-remote 获取远程标签（更可靠）
        self.log("方法1: 使用 git ls-remote 获取远程标签");
        let output = StdCommand::new("git")
            .args(&["ls-remote", "--tags", "--refs", "origin"])
            .current_dir(&self.program_dir)
            .output()?;

        let mut versions_method1 = Vec::new();
        if output.status.success() {
            let tags_output = String::from_utf8(output.stdout)?;
            self.log("ls-remote 标签输出:");
            for line in tags_output.lines() {
                self.log(&format!("{}", line));
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

        // 方法2: 使用本地标签列表（作为备选）
        self.log("方法2: 使用本地标签列表作为备选");
        let output = StdCommand::new("git")
            .args(&["tag", "-l", "--sort=-version:refname"])
            .current_dir(&self.program_dir)
            .output()?;

        let mut versions_method2 = Vec::new();
        if output.status.success() {
            let tags_output = String::from_utf8(output.stdout)?;
            self.log("本地标签输出:");
            for line in tags_output.lines().take(10) { // 只显示前10个
                self.log(&format!("  {}", line));
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

        // 选择获取到更多版本的方法
        let mut versions = if versions_method1.len() >= versions_method2.len() {
            self.log(&format!("使用方法1结果，获取到 {} 个版本", versions_method1.len()));
            versions_method1
        } else {
            self.log(&format!("使用方法2结果，获取到 {} 个版本", versions_method2.len()));
            versions_method2
        };

        if versions.is_empty() {
            return Err(format!("没有找到符合通道 {} 的有效版本", channel.as_str()).into());
        }

        // 排序版本
        versions.sort();
        let latest = versions.last().unwrap();
        self.log(&format!("找到 {} 个有效版本", versions.len()));
        self.log("版本排序结果:");
        for (i, v) in versions.iter().enumerate() {
            let marker = if i == versions.len() - 1 { " -> [最新]" } else { "" };
            self.log(&format!("  {}. {}{}", i + 1, v.raw, marker));
        }
        self.log(&format!("远端最新版本: {}", latest.raw));
        Ok(latest.raw.clone())
    }

    fn kill_process(&self, pid_str: &str) -> Result<(), Box<dyn std::error::Error>> {
        if pid_str == "unset" {
            return Ok(());
        }

        if !self.pid_regex.is_match(pid_str) {
            return Err(format!("Invalid PID format: {}", pid_str).into());
        }

        self.log(&format!("正在关闭进程: {}", pid_str));

        let output = StdCommand::new("kill")
            .args(&["-9", pid_str])
            .output()?;

        if output.status.success() {
            self.log("进程已关闭");
        } else {
            self.log("关闭进程失败或进程不存在");
        }

        Ok(())
    }

    fn clean_installed(&self) -> Result<(), Box<dyn std::error::Error>> {
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

    fn install(&self, channel: &Channel) -> Result<(), Box<dyn std::error::Error>> {
        self.log("开始安装");
        self.log(&format!("安装目录: {:?}, 通道: {}", self.ntsport_dir, channel.as_str()));

        self.clean_installed()?;

        // 创建安装目录
        fs::create_dir_all(&self.ntsport_dir)?;
        self.log(&format!("创建程序安装目录: {:?}", self.ntsport_dir));

        // 获取Git URL
        let git_url = self.get_git_url()?;

        // 克隆仓库
        self.log("正在下载程序");
        let output = StdCommand::new("git")
            .args(&["clone", &git_url, &self.program_dir.to_string_lossy()])
            .current_dir(&self.ntsport_dir)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("Failed to clone repository: {}", stderr).into());
        }

        // 设置安全目录
        let _ = StdCommand::new("git")
            .args(&["config", "--global", "--add", "safe.directory", &self.program_dir.to_string_lossy()])
            .output();

        // 获取并切换到最新版本
        self.fetch_remote()?;
        let latest_version = self.get_latest_version(channel)?;
        self.log(&format!("正在切换到版本: {}", latest_version));

        let output = StdCommand::new("git")
            .args(&["checkout", &latest_version])
            .current_dir(&self.program_dir)
            .output()?;

        if !output.status.success() {
            //尝试使用 reset --hard
            let output = StdCommand::new("git")
                .args(&["reset", "--hard", &latest_version])
                .current_dir(&self.program_dir)
                .output()?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(format!("切换到最新版本失败: {}", stderr).into());
            }
        }

        // 写入配置文件
        fs::write(&self.branch_file, channel.as_str())?;
        self.log(&format!("写入配置文件: channel={}", channel.as_str()));

        self.log(&format!("安装完成! 版本: {}", latest_version));
        Ok(())
    }

    fn upgrade(&self, channel: &Channel) -> Result<(), Box<dyn std::error::Error>> {
        self.log("开始升级");
        self.assert_repos_exist()?;

        // Ensure remote is correct and fetch latest info
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

        // 执行升级
        self.log("正在应用更新");

        // Clean working directory
        let output = StdCommand::new("git")
            .args(&["reset", "--hard"])
            .current_dir(&self.program_dir)
            .output()?;

        if !output.status.success() {
            return Err("Failed to reset git state".into());
        }

        // 切换到最新版本
        self.log(&format!("正在切换到版本: {}", latest_version));
        let output = StdCommand::new("git")
            .args(&["checkout", &latest_version])
            .current_dir(&self.program_dir)
            .output()?;

        if !output.status.success() {
            // 尝试使用 reset --hard
            let output = StdCommand::new("git")
                .args(&["reset", "--hard", &latest_version])
                .current_dir(&self.program_dir)
                .output()?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(format!("Failed to switch to latest version: {}", stderr).into());
            }
        }

        // Update channel config
        fs::write(&self.branch_file, channel.as_str())?;

        self.log(&format!("升级完成! 版本: {} -> {}", current_version, latest_version));
        Ok(())
    }

    fn uninstall(&self) -> Result<(), Box<dyn std::error::Error>> {
        self.log("开始卸载");
        self.clean_installed()?;
        self.log("卸载完成!");
        Ok(())
    }

    fn launch(&self) -> Result<(), Box<dyn std::error::Error>> {
        self.log("启动程序");
        self.assert_repos_exist()?;
        let startup_script = self.program_dir.join("NT.Client.sh");

        if !startup_script.exists() {
            return Err("Startup script not found".into());
        }

        let _child = StdCommand::new("sh")
            .arg(&startup_script)
            .spawn()?;

        self.log("程序已启动");
        Ok(())
    }

    fn create_desktop_entry(&self) -> Result<(), Box<dyn std::error::Error>> {
        let desktop_content = format!(
           r"[Desktop Entry]\n\Type=Application\n\
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
        // Set proper permissions
        let _ = StdCommand::new("chmod")
            .args(&["644", desktop_file.to_str().unwrap()])
            .output();
        self.log("桌面图标创建成功");
        Ok(())
    }

    fn remove_desktop_entry(&self) -> Result<(), Box<dyn std::error::Error>> {
        let desktop_file = Path::new("/usr/share/applications/HoloMotion.desktop");
        if desktop_file.exists() {
            fs::remove_file(desktop_file)?;
            self.log("桌面图标删除成功");
        } else {
            self.log("桌面图标不存在");
        }
        Ok(())
    }

    //添加调试方法来检查当前仓库的标签
    fn debug_list_tags(&self) -> Result<(), Box<dyn std::error::Error>> {
        self.log("=== 调试信息: 当前仓库标签 ===");
        if !self.repos_exist() {
            self.log("仓库不存在");
            return Ok(());
        }
        // 列出本地标签
        let output = StdCommand::new("git")
            .args(&["tag", "-l", "--sort=-version:refname"])
            .current_dir(&self.program_dir)
            .output()?;

        if output.status.success() {
            let tags = String::from_utf8(output.stdout)?;
            self.log("本地标签(按版本排序):");
            for tag in tags.lines().take(20) { // 显示前20个
                self.log(&format!("  {}", tag));
            }
        }
        // 列出远程标签
        let output = StdCommand::new("git")
            .args(&["ls-remote", "--tags", "origin"])
            .current_dir(&self.program_dir)
            .output()?;

        if output.status.success() {
            let tags = String::from_utf8(output.stdout)?;
            self.log("远程标签:");
            for line in tags.lines().take(20) { // 显示前20个
                self.log(&format!("  {}", line));
            }
        }

        // 显示当前分支和提交
        let output = StdCommand::new("git")
            .args(&["branch", "-v"])
            .current_dir(&self.program_dir)
            .output()?;

        if output.status.success() {
            let branches = String::from_utf8(output.stdout)?;
            self.log("当前分支信息:");
            for line in branches.lines() {
                self.log(&format!("  {}", line));
            }
        }

        // 显示最后几次提交
        let output = StdCommand::new("git")
            .args(&["log", "--oneline", "-10"])
            .current_dir(&self.program_dir)
            .output()?;

        if output.status.success() {
            let commits = String::from_utf8(output.stdout)?;
            self.log("最近提交:");
            for line in commits.lines() {
                self.log(&format!("  {}", line));
            }
        }

        self.log("=== 调试信息结束 ===");
        Ok(())
    }

    fn check_status(&self) -> Result<(), Box<dyn std::error::Error>> {
        self.log("=== 系统状态检查 ===");

        // 检查安装状态
        if self.repos_exist() {
            self.log("✓ 应用程序已安装");
            // 检查当前通道
            if let Ok(channel) = self.get_current_channel() {
                self.log(&format!("✓ 当前通道: {}", channel.as_str()));

                // 检查当前版本
                if let Ok(current_version) = self.get_current_version(&channel) {
                    self.log(&format!("✓ 当前版本: {}", current_version));
                    // 检查是否有更新
                    self.ensure_correct_remote()?;
                    self.fetch_remote()?;
                    if let Ok(latest_version) = self.get_latest_version(&channel) {
                        self.log(&format!("✓ 最新版本: {}", latest_version));if current_version == latest_version {
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

        // 检查git配置
        if self.git_file.exists() {
            if let Ok(git_url) = self.get_git_url() {
                self.log(&format!("✓ Git URL 配置: {}", git_url));
            }
        } else {
            self.log("✗ Git URL 配置文件不存在");
        }// 检查启动脚本
        let startup_script = self.program_dir.join("NT.Client.sh");
        if startup_script.exists() {
            self.log("✓ 启动脚本存在");
        } else {
            self.log("✗ 启动脚本不存在");
        }

        self.log("=== 状态检查完成 ===");
        Ok(())
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let matches = Command::new("HoloMotion Installer")
        .version(VERSION)
        .author("HoloMotion Team")
        .about("HoloMotion application installer and updater")
        .arg(Arg::new("channel")
            .short('b')
            .long("channel")
            .value_name("CHANNEL")
            .help("Specify channel: master, release (default)")
            .num_args(1))
        .arg(Arg::new("kill")
            .short('k')
            .value_name("PID")
            .help("Kill process with specified PID before operation")
            .num_args(1))
        .arg(Arg::new("launch")
            .short('r')
            .help("Launch application after install/upgrade")
            .action(clap::ArgAction::SetTrue))
        .subcommand(Command::new("get-current-channel")
            .about("Get current installed channel"))
        .subcommand(Command::new("get-current-version")
            .about("Get current installed version"))
        .subcommand(Command::new("get-latest-version")
            .about("Get latest available version"))
        .subcommand(Command::new("install")
            .about("Install application"))
        .subcommand(Command::new("upgrade")
            .about("Upgrade application"))
        .subcommand(Command::new("uninstall")
            .about("Uninstall application"))
        .subcommand(Command::new("launch")
            .about("Launch application"))
        .subcommand(Command::new("create-desktop")
            .about("Create desktop entry (requires sudo)"))
        .subcommand(Command::new("remove-desktop")
            .about("Remove desktop entry (requires sudo)"))
        .subcommand(Command::new("debug-tags")
            .about("Debug: list all tags in repository"))
        .subcommand(Command::new("status")
            .about("Check system and installation status"))
        .subcommand(Command::new("force-refresh")
            .about("Force refresh remote tags"))
        .get_matches();

    let installer = HoloMotionInstaller::new()?;

    // Handle process killing if specified
    if let Some(pid) = matches.get_one::<String>("kill") {
        installer.kill_process(pid)?;
    }

    // Determine channel
    let channel = if let Some(ch) = matches.get_one::<String>("channel") {
        Channel::from_str(ch)?
    } else {
        // Try to get current channel, default to Release if not found
        installer.get_current_channel().unwrap_or(Channel::Release)
    };

    match matches.subcommand() {
        Some(("get-current-channel", _)) => {
            let current_channel = installer.get_current_channel()?;
            println!("{}", current_channel.as_str());
        }
        Some(("get-current-version", _)) => {
            let version = installer.get_current_version(&channel)?;
            println!("{}", version);
        }
        Some(("get-latest-version", _)) => {
            let version = installer.get_latest_version(&channel)?;
            println!("{}", version);
        }
        Some(("install", _)) => {
            installer.install(&channel)?;if matches.get_flag("launch") {
                installer.launch()?;
            }
        }
        Some(("upgrade", _)) => {
            installer.upgrade(&channel)?;
            if matches.get_flag("launch") {
                installer.launch()?;
            }
        }
        Some(("uninstall", _)) => {
            installer.uninstall()?;
        }
        Some(("launch", _)) => {
            installer.launch()?;
        }
        Some(("create-desktop", _)) => {
            installer.create_desktop_entry()?;
        }
        Some(("remove-desktop", _)) => {
            installer.remove_desktop_entry()?;
        }
        Some(("debug-tags", _)) => {
            installer.debug_list_tags()?;
        }
        Some(("status", _)) => {
            installer.check_status()?;
        }
        Some(("force-refresh", _)) => {
            installer.force_refresh_tags()?;
        }
        _ => {
            println!("HoloMotion Installer v{} - {}", VERSION, PUBLISH_DATE);
            println!("使用 --help 查看使用说明");}
    }

    Ok(())
}