from __future__ import annotations

import json
from pathlib import Path
import tempfile
import unittest

from mcp.server import APP_MIME_TYPE, APP_RESOURCE_URI, MessageParser, OpenMontageMcpServer


class McpServerTest(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.server = OpenMontageMcpServer(projects_root=Path(self.temp.name))

    def tearDown(self) -> None:
        self.temp.cleanup()

    def request(self, method: str, params: dict | None = None) -> dict:
        response = self.server.handle_request({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params or {},
        })
        assert response is not None
        self.assertNotIn("error", response)
        return response["result"]

    def test_initialize_exposes_tools_and_resources(self) -> None:
        result = self.request("initialize", {"protocolVersion": "2024-11-05"})
        self.assertEqual(result["serverInfo"]["name"], "openmontage-mcp")
        self.assertEqual(result["serverInfo"]["version"], "0.3.2")
        self.assertIn("tools", result["capabilities"])
        self.assertIn("resources", result["capabilities"])
        self.assertIn("简体中文", result["instructions"])

    def test_tools_include_compatibility_and_short_drama_contracts(self) -> None:
        result = self.request("tools/list")
        names = {item["name"] for item in result["tools"]}
        self.assertTrue({"env_check", "run_tool", "register_asset"}.issubset(names))
        self.assertTrue({
            "drama_project_create",
            "drama_stage_context",
            "drama_artifact_save",
            "drama_open_review",
            "drama_selection_commit",
            "drama_stage_decide",
        }.issubset(names))
        open_review = next(item for item in result["tools"] if item["name"] == "drama_open_review")
        self.assertEqual(open_review["_meta"]["ui"]["resourceUri"], APP_RESOURCE_URI)

    def test_resource_returns_self_contained_mcp_app(self) -> None:
        listed = self.request("resources/list")
        self.assertEqual(listed["resources"][0]["uri"], APP_RESOURCE_URI)
        read = self.request("resources/read", {"uri": APP_RESOURCE_URI})
        resource = read["contents"][0]
        self.assertEqual(resource["mimeType"], APP_MIME_TYPE)
        self.assertIn("OpenMontage AI 短剧工作台", resource["text"])
        self.assertEqual(resource["_meta"]["ui"]["csp"]["connectDomains"], [])

    def test_create_project_returns_structured_content_and_ui_metadata(self) -> None:
        result = self.request("tools/call", {
            "name": "drama_project_create",
            "arguments": {
                "projectId": "mcp-drama-001",
                "title": "MCP 短剧",
                "sourceText": "一个人在午夜收到未来的电话。",
            },
        })
        self.assertFalse(result.get("isError", False))
        self.assertEqual(result["structuredContent"]["projectId"], "mcp-drama-001")
        self.assertEqual(result["structuredContent"]["revision"], 2)
        self.assertEqual(result["_meta"]["ui"]["resourceUri"], APP_RESOURCE_URI)

    def test_unknown_tool_is_structured_error(self) -> None:
        result = self.request("tools/call", {"name": "missing_tool", "arguments": {}})
        self.assertTrue(result["isError"])
        self.assertEqual(result["structuredContent"]["error"]["code"], "UNKNOWN_TOOL")

    def test_message_parser_supports_jsonl_and_content_length(self) -> None:
        jsonl = MessageParser()
        message = {"jsonrpc": "2.0", "id": 1, "method": "ping"}
        parsed = jsonl.push((json.dumps(message) + "\n").encode())
        self.assertEqual(parsed, [message])
        self.assertTrue(jsonl.frame(message).endswith(b"\n"))

        header = MessageParser()
        body = json.dumps(message).encode()
        framed = f"Content-Length: {len(body)}\r\n\r\n".encode() + body
        self.assertEqual(header.push(framed), [message])
        self.assertTrue(header.frame(message).startswith(b"Content-Length:"))


if __name__ == "__main__":
    unittest.main()
