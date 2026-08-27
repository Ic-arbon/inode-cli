# inode-vpn v1.0.0 发布检查清单

发布前置：`docs/acceptance-runbook.md` 的 A1–A40 全部 PASS，并留有记录。

## 代码与构建

- [ ] `dev/inode-vpn` 分支 HEAD 干净（除 `.auth`/`.auth.example` 等本地文件）
- [ ] `nix flake check` 四平台通过（x86_64-linux / aarch64-linux / x86_64-darwin / aarch64-darwin）
- [ ] `nix build .#inode .#inode-openconnect` 通过
- [ ] `cargo test --workspace` 全绿
- [ ] `cargo clippy --workspace --all-targets` 无 error（warning 可记录）
- [ ] CI runner 记录四平台构建结果

## 安全

- [ ] 日志 / diagnose / status 输出 grep 不到密码与 `svpnginfo` cookie
- [ ] 配置 0600 强制（0644 拒绝启动）单测通过
- [ ] systemd 安全项验收通过（`systemd-analyze security` 无高危）
- [ ] pin 变更需 `--force`（`discover-cert` 行为已实测）
- [ ] 管理 socket peer uid 拒绝非属主连接已实测

## 真机验收

- [ ] Linux M3：A31–A37 记录齐全
- [ ] macOS M4：A31–A33、A36、A38 记录齐全
- [ ] 24h soak 两台各一份记录

## 文档与兼容

- [ ] `docs/architecture.md` 与代码一致
- [ ] `docs/acceptance-runbook.md` 可用
- [ ] `README.md` 覆盖安装 / 迁移 / 使用 / 安全
- [ ] legacy `vpn` shell / `vpn-watch` / `vpn-inode` 已从 flake 移除（v1.0 只提供 `inode`）

## 发布操作

```sh
git checkout dev/inode-vpn
git pull --ff-only
nix flake check --all-systems
git tag -a v1.0.0 -m "inode-vpn v1.0.0"
git push origin v1.0.0 dev/inode-vpn
```

- [ ] tag `v1.0.0` 已推送
- [ ] 在发布说明中附上验收记录摘要
