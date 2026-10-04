class Spill < Formula
  desc "Keep large MCP tool results out of the context window"
  homepage "https://github.com/spill-ai/spill"
  url "https://github.com/spill-ai/spill/archive/refs/tags/v0.1.0.tar.gz"
  sha256 "ebbe949a7625be16cb32acf0a17def32731c5b6240db0f91769265a660aaae99"
  version "0.1.0"
  license "Apache-2.0"
  head "https://github.com/spill-ai/spill.git", branch: "main"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args
  end

  test do
    ENV["HOME"] = testpath.to_s
    ENV.delete("CODEX_HOME")
    ENV.delete("CLAUDE_CONFIG_DIR")
    assert_match version.to_s, shell_output("#{bin}/spill --version")
    assert_equal [], JSON.parse(shell_output("#{bin}/spill list"))

    system bin/"spill", "install", "cursor"
    assert_path_exists testpath/".cursor/mcp.json"

    payload = [{ "id" => 1, "body" => "x" * 40_000 }].to_json
    event = { "tool_name" => "MCP:brew_test", "tool_output" => payload }.to_json
    output = JSON.parse(pipe_output("#{bin}/spill hook cursor", event, 0))
    assert output.key?("updated_mcp_tool_output")

    dataset = JSON.parse(shell_output("#{bin}/spill list")).first.fetch("dataset")
    result = JSON.parse(shell_output("#{bin}/spill sql 'SELECT count(*) FROM #{dataset}'"))
    assert_equal [[1]], result.fetch("rows")

    system bin/"spill", "uninstall", "cursor"
    servers = JSON.parse((testpath/".cursor/mcp.json").read).fetch("mcpServers")
    refute servers.key?("spill")

    %w[codex claude].each do |client|
      system bin/"spill", "install", client
      system bin/"spill", "uninstall", client
    end
  end
end
