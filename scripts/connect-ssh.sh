#!/usr/bin/env bash
# Passwords stay in system SSH. The app only reuses its private control socket.
set -euo pipefail
export LC_ALL=C
action=connect
case "${1:-}" in
  --check) action=check; shift ;;
  --disconnect) action=exit; shift ;;
esac
if [[ $# -lt 2 || $# -gt 3 ]]; then
  echo '用法: connect-ssh.sh [--check|--disconnect] HOST USER [PORT]' >&2
  exit 2
fi
remote_host=$1
remote_user=$2
remote_port=${3:-22}
if [[ ${#remote_host} -gt 253 || ! $remote_host =~ ^[A-Za-z0-9]([A-Za-z0-9.-]*[A-Za-z0-9])?$ ||
      ${#remote_user} -gt 64 || ! $remote_user =~ ^[A-Za-z0-9_][A-Za-z0-9_-]*$ ||
      ! $remote_port =~ ^[0-9]{1,5}$ ]] || (( 10#$remote_port < 1 || 10#$remote_port > 65535 )); then
  echo '主机、用户名或端口无效；使用 IPv4/DNS 和 1–65535 端口。' >&2
  exit 2
fi
IFS=. read -r -a host_labels <<< "$remote_host"
for label in "${host_labels[@]}"; do
  if [[ -z $label || ${#label} -gt 63 || $label == -* || $label == *- ]]; then
    echo '主机名格式无效。' >&2
    exit 2
  fi
done
ssh_options=(-F /dev/null -S "$HOME/.ssh/lan-remoteapp-%C" -T
  -oStrictHostKeyChecking=yes -oConnectTimeout=8 -oClearAllForwardings=yes
  -oForwardAgent=no -oForwardX11=no -oPermitLocalCommand=no
  -p "$((10#$remote_port))" -l "$remote_user")
if [[ $action != connect ]]; then
  exec /usr/bin/ssh "${ssh_options[@]}" -O "$action" -- "$remote_host"
fi
if /usr/bin/ssh "${ssh_options[@]}" -O check -- "$remote_host" >/dev/null 2>&1; then
  echo '已有项目 SSH 登录会话，可直接使用启动器。'
  exit 0
fi
if [[ ! -d $HOME/.ssh ]]; then mkdir -m 700 "$HOME/.ssh"; fi
/usr/bin/ssh "${ssh_options[@]}" -M -N -f -oControlPersist=10m -- "$remote_host"
echo 'SSH 登录已建立；空闲 10 分钟后自动结束。现在可使用启动器。'
