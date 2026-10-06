#!/bin/sh
# Print the Homebrew formula for one release of sessionkit.
#
# Usage: formula.sh <version> <arm asset API URL> <arm sha256> <intel asset API URL> <intel sha256>
#
# The repository is private, so the formula downloads each binary through the GitHub API with
# the token Homebrew already uses: $HOMEBREW_GITHUB_API_TOKEN, else the gh login, else the
# keychain. GitHub answers with a redirect to a signed URL on another host; Homebrew drops the
# headers there, so the token goes to api.github.com only.
set -eu
version=$1 arm_url=$2 arm_sha=$3 intel_url=$4 intel_sha=$5

cat <<EOF
# frozen_string_literal: true

require "download_strategy"
require "utils/github/api"

# Downloads a release asset of a private GitHub repository through the API.
class GitHubPrivateReleaseDownloadStrategy < CurlDownloadStrategy
  # The install step runs in a sandbox that cannot reach the gh login; it needs no token either,
  # because the download is done by then.
  def initialize(url, name, version, **meta)
    token = GitHub::API.credentials
    meta[:headers] = ["Authorization: token #{token}", "Accept: application/octet-stream"] if token.present?
    super
  end

  def fetch(timeout: nil)
    odie "sessionkit is private: run gh auth login, or set HOMEBREW_GITHUB_API_TOKEN." if meta[:headers].blank?
    super
  end
end

# sessionkit: start, measure and analyse agent sessions. Written by the release workflow.
class Sessionkit < Formula
  desc "Start, measure and analyse agent sessions"
  homepage "https://github.com/Calq-dev/sessionkit"
  version "${version}"

  if Hardware::CPU.arm?
    url "${arm_url}", using: GitHubPrivateReleaseDownloadStrategy
    sha256 "${arm_sha}"
  else
    url "${intel_url}", using: GitHubPrivateReleaseDownloadStrategy
    sha256 "${intel_sha}"
  end

  depends_on :macos

  def install
    bin.install "sessionkit"
  end

  def caveats
    <<~TEXT
      Run once to add the status line and hooks to Claude Code:
        sessionkit setup
    TEXT
  end

  test do
    assert_match "sessionkit", shell_output("#{bin}/sessionkit --help")
  end
end
EOF
