class Malac < Formula
  desc "Multi-Agent Loop for Autonomous Coding"
  homepage "https://github.com/lobirus/malac"
  url "https://github.com/lobirus/malac/archive/refs/tags/v1.2.5.tar.gz"
  sha256 "8a41847decfba77879ebcc51bb472431dea163f4f060a07aacb3cf6d4c7b377f"
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
