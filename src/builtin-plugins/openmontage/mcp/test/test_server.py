from __future__ import annotations

import json
import importlib.util
import sys
import unittest
from pathlib import Path


MCP_ROOT = Path(__file__).resolve().parents[1]
SERVER_PATH = MCP_ROOT / "server.py"
spec = importlib.util.spec_from_file_location("openmontage_mcp_server", SERVER_PATH)
assert spec and spec.loader
server = importlib.util.module_from_spec(spec)
spec.loader.exec_module(server)


class OpenMontageMcpTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.server = server.OpenMontageMcpServer()

    def request(self, method: str, params: dict | None = None) -> dict:
        response = self.server.handle_request(
            {"jsonrpc": "2.0", "id": 1, "method": method, "params": params or {}}
        )
        self.assertIsNotNone(response)
        self.assertNotIn("error", response)
        return response["result"]

    def test_initialize(self) -> None:
        result = self.request("initialize", {"protocolVersion": "2024-11-05"})
        self.assertEqual(result["serverInfo"]["name"], "openmontage-mcp")
        self.assertEqual(result["serverInfo"]["version"], "0.3.2")
        self.assertIn("简体中文", result["instructions"])
        self.assertIn("火山引擎", result["instructions"])

    def test_tools_list(self) -> None:
        result = self.request("tools/list")
        names = sorted(tool["name"] for tool in result["tools"])
        self.assertTrue(
            {
                "env_check",
                "get_pipeline",
                "list_pipelines",
                "list_tools",
                "provider_menu",
                "register_asset",
                "run_tool",
                "tool_info",
            }.issubset(names)
        )

    def test_unknown_tool_is_structured_error(self) -> None:
        result = self.request("tools/call", {"name": "not_a_tool", "arguments": {}})
        self.assertTrue(result["isError"])
        payload = json.loads(result["content"][0]["text"])
        self.assertEqual(payload["error"]["code"], "UNKNOWN_TOOL")

    def test_pipeline_path_is_restricted(self) -> None:
        result = self.request("tools/call", {"name": "get_pipeline", "arguments": {"name": "../x.yaml"}})
        self.assertTrue(result["isError"])
        payload = json.loads(result["content"][0]["text"])
        self.assertEqual(payload["error"]["code"], "PIPELINE_NAME_REQUIRED")


if __name__ == "__main__":
    unittest.main()
