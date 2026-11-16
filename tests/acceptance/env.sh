# Acceptance settings, sourced by run.sh. Network values are the mini's, from plan 1's spike;
# override any of them in the environment.
ACC=${ACC:-$HOME/.bunker-acceptance}
export BUNKER_HOME=$ACC/home                      # never the real ~/.bunker
export BUNKER_KEYCHAIN=$ACC/acceptance.keychain-db  # never the login keychain
export LIMA_HOME=${LIMA_HOME:-$HOME/.lima}
BUNKER=$ACC/bin/bunker                            # a copy, so a rebuild mid-run changes nothing
PROJECT=fixture
VM=bunker-$PROJECT
MAC_LAN=${MAC_LAN:-192.168.254.114}
ROUTER=${ROUTER:-192.168.254.254}
TS_PEER=${TS_PEER:-100.80.217.65}
TS_SELF=${TS_SELF:-100.112.139.61}
LOOP_PORT=${LOOP_PORT:-18080}   # a listener on the host's loopback the guest must not reach
ALL_PORT=${ALL_PORT:-18081}     # a listener on all host addresses, probed by the LAN address
IMAGE_CACHE=${IMAGE_CACHE:-$HOME/.bunker/images}   # verified images to clone instead of downloading
FIXTURE_TARBALL=${FIXTURE_TARBALL:-https://github.com/laravel/laravel/archive/refs/heads/12.x.tar.gz}
