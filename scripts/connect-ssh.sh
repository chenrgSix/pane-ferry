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
  -oServerAliveInterval=10 -oServerAliveCountMax=2
  -p "$((10#$remote_port))" -l "$remote_user")
if [[ $action == exit ]]; then
  exec /usr/bin/ssh "${ssh_options[@]}" -O "$action" -- "$remote_host"
fi
master_exists() {
  /usr/bin/ssh "${ssh_options[@]}" -O check -- "$remote_host" >/dev/null 2>&1
}
transport_responds() {
  # A mux PID can survive a broken TCP connection. Probe an authenticated
  # channel, with a local deadline that also applies to a stuck old master.
  /usr/bin/ssh "${ssh_options[@]}" -n -oBatchMode=yes -- "$remote_host" /usr/bin/true >/dev/null 2>&1 &
  local probe_pid=$!
  ( sleep 10; kill -TERM "$probe_pid" 2>/dev/null || : ) &
  local timer_pid=$!
  local result=0
  wait "$probe_pid" || result=$?
  kill "$timer_pid" 2>/dev/null || :
  wait "$timer_pid" 2>/dev/null || :
  return "$result"
}
if master_exists; then
  if transport_responds; then
    echo '已有可用的 SSH 登录会话，可直接使用启动器。'
    exit 0
  fi
  if [[ $action == check ]]; then
    echo 'SSH 登录进程仍在，但传输无响应；请重新登录 SSH。' >&2
    exit 1
  fi
  echo '旧 SSH 传输已失效，正在重新建立登录。'
  /usr/bin/ssh "${ssh_options[@]}" -O exit -- "$remote_host" >/dev/null 2>&1 || :
elif [[ $action == check ]]; then
  echo '没有可用的临时 SSH 登录会话，请重新登录 SSH。' >&2
  exit 1
fi
if [[ ! -d $HOME/.ssh ]]; then mkdir -m 700 "$HOME/.ssh"; fi
/usr/bin/ssh "${ssh_options[@]}" -oControlMaster=auto -N -f -oControlPersist=10m -- "$remote_host"
echo 'SSH 登录已建立；空闲 10 分钟后自动结束。现在可使用启动器。'
