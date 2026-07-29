#!/usr/bin/env bash
# Pod-side provisioning for HyperFrames rendering. Idempotent — safe to re-run.
# Piped in by `podctl.sh provision`; runs as root on runpod/base:0.6.2-cpu
# (Ubuntu 20.04, ffmpeg preinstalled).
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive

echo "bootstrap: $(nproc) vCPU, $(free -g | awk '/^Mem:/{print $2}')GB RAM"

if ! command -v node >/dev/null 2>&1 || [ "$(node -p 'process.versions.node.split(".")[0]' 2>/dev/null || echo 0)" -lt 22 ]; then
  echo "bootstrap: installing Node 22..."
  curl -fsSL https://deb.nodesource.com/setup_22.x -o /tmp/nodesource.sh
  bash /tmp/nodesource.sh >/dev/null 2>&1
  apt-get install -y nodejs >/dev/null 2>&1
else
  echo "bootstrap: node $(node --version) already present"
fi

if ! command -v google-chrome >/dev/null 2>&1; then
  echo "bootstrap: installing Chrome..."
  apt-get update -qq >/dev/null 2>&1
  curl -fsSL -o /tmp/chrome.deb https://dl.google.com/linux/direct/google-chrome-stable_current_amd64.deb
  apt-get install -y -qq /tmp/chrome.deb fonts-liberation >/dev/null 2>&1
else
  echo "bootstrap: $(google-chrome --version) already present"
fi

command -v ffmpeg >/dev/null 2>&1 || { apt-get update -qq >/dev/null 2>&1; apt-get install -y -qq ffmpeg >/dev/null 2>&1; }

# HyperFrames' own Chrome download fails in minimal containers (no zip archiver),
# so always point it at the system Chrome.
grep -q HYPERFRAMES_BROWSER_PATH /root/.bashrc 2>/dev/null || \
  echo 'export HYPERFRAMES_BROWSER_PATH=/usr/bin/google-chrome' >> /root/.bashrc

python3 -c "import numpy, PIL" >/dev/null 2>&1 || {
  echo "bootstrap: installing numpy/pillow for zone analysis..."
  apt-get install -y -qq python3-numpy python3-pil >/dev/null 2>&1
}

mkdir -p /root/job
echo "bootstrap: ready — node $(node --version), $(google-chrome --version), $(ffmpeg -version | head -1 | cut -d' ' -f1-3)"
