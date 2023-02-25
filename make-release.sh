#!/bin/bash
#
# Make sure `cross` is installed.
# You'll also need `sed`, a relatively recent version of `tar`, and `7z`.
DOCKER="docker"
#
shopt -s extglob
# Trap errors and interrupts
set -Eeuo pipefail
function handle_sigint() {
  echo "SIGINT, exiting..."
  exit 1
}
trap handle_sigint SIGINT
function handle_err() {
  echo "Error in run.sh!" 1>&2
  echo "$(caller): ${BASH_COMMAND}" 1>&2
  echo "Exiting..."
  exit 2
}
trap handle_err ERR

# Go to the root of the project
SCRIPT=$(realpath "${0}")
SCRIPTPATH=$(dirname "${SCRIPT}")
cd "${SCRIPTPATH}" || exit 12

declare -A TARGETS=(
  ['x86_64-unknown-linux-musl']='linux-x86_64'
  ['x86_64-pc-windows-gnu']='windows-x86_64'
  ['aarch64-unknown-linux-musl']='linux-aarch64'
  ['armv7-unknown-linux-musleabihf']='linux-armv7'
  ['arm-unknown-linux-musleabihf']='linux-armv6'
)

declare -A DOCKER_TARGETS=(
  ['x86_64-unknown-linux-musl']='linux/amd64'
  ['aarch64-unknown-linux-musl']='linux/arm64'
  ['armv7-unknown-linux-musleabihf']='linux/arm/v7'
  ['arm-unknown-linux-musleabihf']='linux/arm/v6'
)

prompt_confirm() {
  while true; do
    read -r -n 1 -p "${1} [y/N]: " REPLY
    case $REPLY in
    [yY] | [Yy][Ee][Ss]) return 0 ;;
    *) exit 1 ;;
    esac
  done
}

# Get the version number
VERSION=$(sed -nr 's/^version *= *"([0-9.]+)"/\1/p' Cargo.toml)
prompt_confirm "Releasing version ${VERSION}, please make sure all Cargo.toml and package.json files are updated."

# Build the CSS
yarn style:build

# Make the builds
for target in "${!TARGETS[@]}"; do
  echo Building "${target}"
  # Keeping the cached builds seem to be breaking things when going between targets
  # This wouldn't be a problem if these were running in a matrix on the CI...
  rm -rf target/release/
  cross build -j $(($(nproc) / 2)) --release --target "${target}"
  if [[ "${target}" =~ .*"windows".* ]]; then
    zip -j "http-drogue.${VERSION}.${TARGETS[${target}]}.zip" target/"${target}"/release/http-drogue.exe 1>/dev/null
  else
    tar -acf "http-drogue.${VERSION}.${TARGETS[${target}]}.tar.xz" -C "target/${target}/release/" "http-drogue"
  fi
done

if [[ "$#" -ge 2 && "$1" = "--no-docker" ]]; then
  echo "Exiting without releasing to dockerhub"
  exit 0
fi

# Copy files into place so Docker can get them easily
mkdir -p Docker
pushd Docker
echo Building Docker images
mkdir -p binaries
for target in "${!DOCKER_TARGETS[@]}"; do
  mkdir -p "binaries/${DOCKER_TARGETS[${target}]}"
  cp ../target/"${target}"/release/http-drogue?(|.exe) "binaries/${DOCKER_TARGETS[${target}]}/http-drogue"
done

${DOCKER} buildx build . \
  --platform=linux/amd64,linux/arm64,linux/arm/v6,linux/arm/v7 \
  --file "Dockerfile" \
  --tag "seriousbug/http-drogue:latest" \
  --tag "seriousbug/http-drogue:${VERSION}" \
  --push
popd
