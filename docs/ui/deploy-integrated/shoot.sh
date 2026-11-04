#!/bin/sh
# Screenshots of the INTEGRATED desktop UI (simulated Deployments) via the Vite dev server.
# Start it first:  cd ui/harpoon-desktop && npx vite --port 1420 --strictPort
# Usage: sh docs/ui/deploy-integrated/shoot.sh   (mirrors docs/ui/deploy-prototype/shoot.sh states)
CH="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
cd "$(dirname "$0")" && mkdir -p shots
shot() { "$CH" --headless=new --disable-gpu --hide-scrollbars --force-device-scale-factor=1 --window-size=$2 --virtual-time-budget=5000 --screenshot="$PWD/shots/$1.png" "http://localhost:1420/#deploy=$3" >/dev/null 2>&1 && echo "$1"; }
shot 01-projects-attention 1440,900 "scenario:cutover-restored,screen:home,runtime:connected"
shot 02-app-healthy 1440,900 "scenario:healthy,screen:app,tab:overview,runtime:connected"
shot 03-app-cutover-restored 1440,1100 "scenario:cutover-restored,screen:app,tab:overview,runtime:connected"
shot 04-app-down 1440,1100 "scenario:restore-failed,screen:app,tab:overview,runtime:connected"
shot 05-app-recovering 1440,1100 "scenario:recovering,screen:app,tab:overview,runtime:connected"
shot 06-deployments-history 1440,900 "scenario:build-failed,screen:app,tab:deployments,runtime:connected"
shot 07-deployment-build-failed 1440,1200 "scenario:build-failed,screen:dep,dep:d-43,runtime:connected"
shot 08-deployment-recovery-failed 1440,1400 "scenario:restore-failed,screen:dep,dep:d-43,runtime:connected"
shot 09-logs-live-build 1440,800 "scenario:building,screen:app,tab:logs,runtime:connected"
shot 10-metrics 1440,1000 "scenario:healthy,screen:app,tab:metrics,runtime:connected"
shot 11-variables-keychain-locked 1440,1100 "scenario:healthy,screen:app,tab:variables,keychain:locked,runtime:connected"
shot 12-settings 1440,1100 "scenario:healthy,screen:app,tab:settings,runtime:connected"
shot 13-create-source 1440,900 "scenario:healthy,screen:home,dialog:create,runtime:connected"
shot 14-resume-automation 1440,900 "scenario:cutover-restored,screen:app,dialog:resume,runtime:connected"
shot 15-rollback-confirm 1440,900 "scenario:healthy,screen:app,dialog:rollback,runtime:connected"
shot 16-deploy-plan-keychain-locked 1440,900 "scenario:healthy,screen:app,dialog:deploy,keychain:locked,runtime:connected"
shot 17-empty 1440,800 "scenario:empty,screen:home,runtime:connected"
shot 18-loading 1440,700 "scenario:loading,screen:home,runtime:connected"
shot 19-runtime-stopped 1440,1000 "scenario:healthy,screen:app,tab:overview,runtime:stopped"
shot 21-compact-1100 1100,900 "scenario:cutover-restored,screen:app,tab:overview,runtime:connected"
shot 22-narrow-860 860,1200 "scenario:cutover-restored,screen:app,tab:overview,runtime:connected"
shot 23-minimum-700 700,1300 "scenario:build-failed,screen:dep,dep:d-43,runtime:connected"
