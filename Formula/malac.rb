class Malac < Formula
  desc "Multi-Agent Loop for Autonomous Coding"
  homepage "https://github.com/lobirus/malac"
  url "https://github.com/lobirus/malac/archive/refs/tags/v0.1.2.tar.gz"
  sha256 "f16a8620d3b35285984813559b693f7dd736177c3a757f8e6b75f86400519197"
  license "MIT"
  head "https://github.com/lobirus/malac.git", branch: "main"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args(path: "crates/malac")
    system "cargo", "install", *std_cargo_args(path: "crates/malac-control")
  end

  def caveats
    <<~EOS
      Add a model provider, then start a session:
        malac provider add <name> --base-url <url> --model <model> --api-key-stdin --set-default
        malac

      For desktop control on macOS, grant Screen Recording and Accessibility
      to your terminal app in System Settings > Privacy & Security.
    EOS
  end

  test do
    assert_match "malac", shell_output("#{bin}/malac --version")
    assert_match "Usage", shell_output("#{bin}/malac-control --help")
  end
end
