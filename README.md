# openconnect-h3c VPN

带 H3C SSL VPN 协议支持的 OpenConnect，通过 Nix flake 统一构建，并附带一个仿 systemd 用法的 `vpn` 管理命令（连接 / 断开 / 状态 / macOS 开盖唤醒自动重连）。

## 1. 安装 Nix

需要支持 flakes 的 Nix。推荐用 [Determinate Systems 安装器](https://github.com/DeterminateSystems/nix-installer)，它默认开启 flakes，且卸载干净：

```sh
curl --proto '=https' --tlsv1.2 -sSf -L https://install.determinate.systems/nix | sh -s -- install
```

装完后**重开一个终端**，验证：

```sh
nix --version
```

> 如果你用的是官方安装器，需要手动开启 flakes，在 `~/.config/nix/nix.conf` 写入：
> ```
> experimental-features = nix-command flakes
> ```

## 2. 配置凭据

复制示例并填入你的账号信息：

```sh
cp .auth.example .auth
```

`.auth` 为 `key=value` 格式（已在 `.gitignore` 中，不会被提交）：

```
username=你的用户名
password=你的密码
servercert=pin-sha256:...        # 可选，证书指纹校验
gateway=vpn.example.com:443      # 网关地址:端口
```

## 3. 进入开发环境

在项目目录下：

```sh
nix develop
```

首次会拉取并构建 openconnect-h3c（耗时较长，后续走缓存秒进）。进入后会打印可用命令提示，此时 `vpn`、`openconnect` 都已在 `PATH` 中。

### 可选：用 direnv 自动进入

仓库已带 `.envrc`（内容为 `use flake`）。装好 [direnv](https://direnv.net/) 后，在目录里执行一次授权，之后 `cd` 进来即自动加载环境：

```sh
direnv allow
```

### 不进 shell 直接跑

```sh
nix run .#vpn -- start
```

## 4. 使用 `vpn` 命令

仿 systemd 用法，需在含 `.auth` 的目录中运行：

```sh
vpn start             # 连接
vpn stop              # 断开
vpn restart           # 当前目录下重连
vpn status            # 查看连接 / 工作目录 / 自动重连状态
```

### macOS 开盖唤醒自动重连（可选）

```sh
vpn install-sudoers   # 一次性写入 openconnect 的 sudo 免密规则（需一次 sudo 授权）
vpn enable            # 安装开盖唤醒自动重连服务（依赖上面的免密）
vpn enable --now      # 安装服务并立即连接
vpn disable [--now]   # 移除服务（--now 同时断开）
vpn uninstall-sudoers # 移除免密规则
```

> 自动重连依赖 sudo 免密：唤醒时无人值守，若未配置免密会卡在等待密码而失败。
