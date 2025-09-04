#!/bin/bash

# A script to update the Rust toolchain, project dependencies,
# and check a forked crate against the official version.

# --- Color Codes for Better Output ---
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color

echo -e "${GREEN}--- Starting Rust Project Update ---\n${NC}"

# --- 1. Update the Rust Compiler and Toolchain ---
echo -e "${YELLOW}Step 1: Updating Rust toolchain with 'rustup'...${NC}"
rustup update
if [ $? -ne 0 ]; then
    echo -e "${RED}Error: 'rustup update' failed. Please check your Rust installation.${NC}"
    exit 1
fi
echo -e "${GREEN}Rust toolchain is up to date.\n${NC}"


# --- 2. Update Project Dependencies (Crates) ---
echo -e "${YELLOW}Step 2: Updating project dependencies with 'cargo update'...${NC}"
# This will respect the version lock (=0.17.3) for rodio in Cargo.toml
cargo update
if [ $? -ne 0 ]; then
    echo -e "${RED}Error: 'cargo update' failed. Check for dependency conflicts.${NC}"
    exit 1
fi
echo -e "${GREEN}Project dependencies are up to date.\n${NC}"


# --- 3. Check the Forked 'twitch-irc' Library ---
echo -e "${YELLOW}Step 3: Checking 'twitch-irc' library version...${NC}"

# Define the path to your local fork's Cargo.toml
LOCAL_FORK_CARGO_TOML="twitch-irc_local/Cargo.toml"

if [ ! -f "$LOCAL_FORK_CARGO_TOML" ]; then
    echo -e "${RED}Error: Could not find local fork's config at '$LOCAL_FORK_CARGO_TOML'.${NC}"
    exit 1
fi

# Extract version from your local fork's Cargo.toml
LOCAL_VERSION=$(grep '^version =' "$LOCAL_FORK_CARGO_TOML" | sed 's/version = "\(.*\)"/\1/')

# Get the latest version from crates.io
# We use awk to grab the version number reliably from the 'cargo search' output.
LATEST_VERSION=$(cargo search twitch-irc | grep '^twitch-irc =' | awk '{print $3}' | tr -d '"')


if [ -z "$LATEST_VERSION" ]; then
    echo -e "${RED}Warning: Could not determine the latest version of 'twitch-irc' from crates.io.${NC}"
else
    echo "Your local 'twitch-irc' version: ${LOCAL_VERSION}"
    echo "Latest 'twitch-irc' on crates.io: ${LATEST_VERSION}"

    if [ "$LOCAL_VERSION" == "$LATEST_VERSION" ]; then
        echo -e "${GREEN}Your local fork is up-to-date with the official release.${NC}"
    else
        echo -e "${YELLOW}A new version of 'twitch-irc' is available! You may want to update your fork.${NC}"
    fi
fi

echo -e "\n${GREEN}--- Update Check Complete! ---${NC}"

