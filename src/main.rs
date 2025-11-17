mod server3;

use log;
use server3::sc_send;
use std::env;
use std::error::Error;
use std::path::PathBuf;
use std::process::{Command, exit};
use std::time::Duration;
use tracing::Level;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // 只有注册 subscriber 后， 才能在控制台上看到日志输出
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO) // 仅INFO、WARN、ERROR Level的日志会被打印
        .init();
    // 从 .env 文件加载环境变量。
    // 如果 .env 文件找不到、不可读或无效，则加载失败。
    load_env();
    let maa_bin = env::var("MAA_BIN").expect("请在.env文件里设置MAA的二进制路径");
    let container_name = env::var("CONTAINER_NAME").expect("请在.env文件里设置容器名");
    let adb_target = env::var("ADB_TARGET").expect("请在.env文件里设置adb路径");
    let maa_task_config =
        env::var("MAA_TASK_CONFIG").expect("请在.env文件里设置MAA任务配置文件路径");
    let user_name = env::var("USER_NAME").expect("请在.env文件里设置安装MAA的用户名");
    let user_home = format!("/home/{}", user_name);
    let maa_lib_dir = format!("{}/.local/share/maa/lib", user_home); // 你确认的库目录
    let maa_state_dir = format!("{}/.local/state", user_home);
    let maa_data_dir = format!("{}/.local/share", user_home);
    let maa_config_dir = format!("{}/.config", user_home);
    match sc_send("archMAA".to_string(), "MAA服务准备启动".to_string()).await {
        Ok(_ret) => tracing::info!("Server3酱消息推送成功"),
        Err(_e) => tracing::error!("Server3酱消息推送失败"),
    }

    // 这样可以执行命令
    // let child = Command::new("pwd").output().expect("failed to execute process");
    let podman = String::from_utf8(
        Command::new("podman")
            .arg("ps")
            .arg("-a")
            .output()
            .expect("podman command failed to start")
            .stdout,
    )
    .expect("command not found");

    // 首先确保容器是存在的
    match podman.find(container_name.as_str()) {
        Some(_podman) => {
            log::info!("已找到运行Arknights的容器");
        }
        None => {
            log::error!(
                "请检查容器是否存在以及.env配置是否正确[提示:你是否使用sudo权限运行该工具?]"
            );
            exit(1);
        }
    }

    // 运行容器
    let podman = String::from_utf8(
        Command::new("podman")
            .arg("start")
            .arg(container_name.as_str())
            .output()
            .expect("podman command failed to start")
            .stdout,
    )
    .expect("command not found");
    if !podman.is_empty() {
        log::info!("容器已启动");
    }

    // 清理adb缓存
    Command::new("adb")
        .arg("kill-server")
        .output()
        .expect("adb command failed to start");

    tokio::time::sleep(Duration::from_secs(1)).await;
    Command::new("adb")
        .arg("start-server")
        .output()
        .expect("adb command failed to start");
    log::info!("adb已重启");

    log::info!("等待5秒钟模拟器开机");
    tokio::time::sleep(Duration::from_secs(5)).await;
    // 连接模拟器设备
    let adb = String::from_utf8(
        Command::new("adb")
            .arg("connect")
            .arg(adb_target.as_str())
            .output()
            .expect("adb command failed to start")
            .stdout,
    )
    .expect("adb command failed to start");
    log::info!("{:?}", adb);

    // 运行MAA

    let mut child = Command::new(maa_bin.as_str())
        .arg("run")
        .arg(maa_task_config.as_str())
        // 设置库路径（只影响子进程）
        .env("LD_LIBRARY_PATH", maa_lib_dir.as_str())
        // 让 maa 看到原始用户的 HOME/USER/XDG_*，避免使用 /root
        .env("HOME", &user_home)
        .env("USER", &user_name)
        .env("XDG_STATE_HOME", maa_state_dir)
        .env("XDG_DATA_HOME", maa_data_dir)
        .env("XDG_CONFIG_HOME", maa_config_dir)
        // 如果 maa 需要工作目录（资源），可设置 current_dir：
        .current_dir(format!("{}/.local/share/maa", user_home))
        .spawn()
        .expect("maa task command failed to start");

    let status = child.wait()?;
    println!("Child exited with: {}", status);
    log::info!("MAA任务执行完毕");

    let podman = String::from_utf8(
        Command::new("podman")
            .arg("stop")
            .arg(container_name.as_str())
            .output()
            .expect("podman command failed to start")
            .stdout,
    );
    log::info!("{:?}", podman);

    match sc_send("archMAA".to_string(), "MAA运行完毕".to_string()).await {
        Ok(_ret) => tracing::info!("Server3酱消息推送成功"),
        Err(_e) => tracing::error!("Server3酱消息推送失败"),
    }

    log::info!("已关闭podman容器");

    Ok(())
}

fn load_env() {
    // 1. 尝试从固定用户配置目录加载：~/.config/easy_maa/.env
        let config_path = PathBuf::from("/home/cn059/.config/easy_maa/.env");

        log::info!("Trying to load env from: {:?}", config_path);

        if config_path.exists() {
            dotenvy::from_path(config_path).expect("Failed to load .env from ~/.config/easy_maa/.env");
            return;
        }


    // 2. 否则回退到当前目录（仅开发模式）
    #[cfg(debug_assertions)]
    {
        dotenvy::dotenv().ok();
    }
}
