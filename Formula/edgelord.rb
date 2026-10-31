class Edgelord < Formula
  desc "Prediction market arbitrage detection and execution CLI"
  homepage "https://github.com/rdekovich/edgelord"
  license any_of: ["MIT", "Apache-2.0"]
  # Development installation until a release containing the correctness fixes exists.
  head "https://github.com/rdekovich/edgelord.git", branch: "main"

  depends_on "cmake" => :build
  depends_on "llvm" => :build
  depends_on "rust" => :build

  def install
    ENV["LIBCLANG_PATH"] = Formula["llvm"].opt_lib.to_s
    system "cargo", "install", *std_cargo_args
  end

  test do
    assert_match "edgelord", shell_output("#{bin}/edgelord --version")
    system bin/"edgelord", "strategies", "list"
  end
end
