<p align="center">
  <img src="src-tauri/icons/logo.svg" alt="Codex Switcher" width="128" height="128">
</p>

<h1 align="center">Codex 多账号切换</h1>

<p align="center">
  一款用于管理多个 OpenAI <a href="https://github.com/openai/codex">Codex CLI</a> 账号的桌面应用<br>
  可快速切换账号、查看配额使用情况，并更方便地管理你的额度
</p>

## 功能特性

- **多账号管理**：在一个界面中添加并管理多个 Codex 账号
- **快速切换**：一键切换当前使用的账号
- **配额监控**：实时查看 5 小时配额和周配额的使用情况
- **双登录模式**：支持 OAuth 登录，也支持导入现有的 `auth.json` 文件

## 安装

### 环境要求

- [Node.js](https://nodejs.org/) (v18+)
- [pnpm](https://pnpm.io/)
- [Rust](https://rustup.rs/)

### 从源码构建

```bash
# 克隆仓库
git clone https://github.com/Lampese/codex-switcher.git
cd codex-switcher

# 安装依赖
pnpm install

# 开发模式运行
pnpm tauri dev

# 构建生产版本
pnpm tauri build
```

构建产物位于 `src-tauri/target/release/bundle/`。

## 免责声明

本工具**仅适用于本人合法持有多个 OpenAI/ChatGPT 账号的个人用户**，目的是帮助用户更方便地管理自己的账号。

**本工具不适用于以下用途：**
- 多人之间共享账号
- 规避 OpenAI 的服务条款
- 任何形式的账号池化或凭据共享

使用本软件即表示你确认自己是添加到应用中的所有账号的合法持有人。作者不对任何滥用行为或违反 OpenAI 服务条款的行为承担责任。
