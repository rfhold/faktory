from __future__ import annotations

import json
import struct
import subprocess
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET
from pathlib import Path
from unittest import mock

from renderer.worker import _validate_facts, _validate_glb, _validate_svg, render


def make_glb(json_payload: bytes, bin_payload: bytes | None = None) -> bytes:
    chunks = struct.pack("<II", len(json_payload), 0x4E4F534A) + json_payload
    if bin_payload is not None:
        chunks += struct.pack("<II", len(bin_payload), 0x004E4942) + bin_payload
    return struct.pack("<4sII", b"glTF", 2, 12 + len(chunks)) + chunks


def set_declared_glb_size(content: bytes) -> bytes:
    return content[:8] + struct.pack("<I", len(content)) + content[12:]


class RendererTests(unittest.TestCase):
    def render_paths(self, root: Path) -> tuple[Path, Path, Path]:
        return root / "model.glb", root / "model.svg", root / "model.json"

    def test_supported_result_types_produce_all_outputs(self) -> None:
        sources = {
            "workplane": "import cadquery as cq\nresult = cq.Workplane('XY').box(1, 2, 3)\n",
            "shape": "import cadquery as cq\nresult = cq.Workplane('XY').sphere(2).val()\n",
            "assembly": (
                "import cadquery as cq\n"
                "result = cq.Assembly(cq.Workplane('XY').cylinder(2, 1))\n"
            ),
        }
        for name, source_text in sources.items():
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                source = root / "model.py"
                glb, svg, facts = self.render_paths(root)
                source.write_text(source_text, encoding="utf-8")

                self.assertIsNone(render(source, glb, svg, facts))
                self.assertTrue(_validate_glb(glb))
                self.assertTrue(_validate_svg(svg))
                self.assertTrue(_validate_facts(facts))
                self.assertEqual(glb.read_bytes()[:4], b"glTF")

    def test_box_facts_are_exact(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "model.py"
            source.write_text(
                "import cadquery as cq\nresult = cq.Workplane('XY').box(2, 3, 5)\n",
                encoding="utf-8",
            )
            glb, svg, facts_path = self.render_paths(root)

            self.assertIsNone(render(source, glb, svg, facts_path))
            self.assertEqual(
                json.loads(facts_path.read_text(encoding="utf-8")),
                {
                    "volume_cubic_millimeters": 30.0,
                    "size_millimeters": {"x": 2.0, "y": 3.0, "z": 5.0},
                },
            )

    def test_translated_nested_assembly_facts_preserve_placements(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "model.py"
            source.write_text(
                "import cadquery as cq\n"
                "child = cq.Assembly(cq.Workplane('XY').box(2, 2, 2))\n"
                "child.add(cq.Workplane('XY').box(1, 1, 1), loc=cq.Location((4, 0, 0)))\n"
                "result = cq.Assembly()\n"
                "result.add(child, loc=cq.Location((0, 3, 5)))\n",
                encoding="utf-8",
            )
            glb, svg, facts_path = self.render_paths(root)

            self.assertIsNone(render(source, glb, svg, facts_path))
            facts = json.loads(facts_path.read_text(encoding="utf-8"))
            self.assertAlmostEqual(facts["volume_cubic_millimeters"], 9.0)
            self.assertEqual(facts["size_millimeters"], {"x": 5.5, "y": 2.0, "z": 2.0})

    def test_overlapping_assembly_components_contribute_independent_volume(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "model.py"
            source.write_text(
                "import cadquery as cq\n"
                "result = cq.Assembly(cq.Workplane('XY').box(2, 2, 2))\n"
                "result.add(cq.Workplane('XY').box(2, 2, 2), "
                "loc=cq.Location((1, 0, 0)))\n",
                encoding="utf-8",
            )
            glb, svg, facts_path = self.render_paths(root)

            self.assertIsNone(render(source, glb, svg, facts_path))
            facts = json.loads(facts_path.read_text(encoding="utf-8"))
            self.assertAlmostEqual(facts["volume_cubic_millimeters"], 16.0)
            self.assertEqual(facts["size_millimeters"], {"x": 3.0, "y": 2.0, "z": 2.0})

    def test_svg_is_deterministic_fixed_size_and_parseable(self) -> None:
        rendered = []
        for _ in range(2):
            with tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                source = root / "model.py"
                source.write_text(
                    "import cadquery as cq\nresult = cq.Workplane('XY').box(1, 2, 3)\n",
                    encoding="utf-8",
                )
                glb, svg, facts = self.render_paths(root)
                self.assertIsNone(render(source, glb, svg, facts))
                rendered.append(svg.read_text(encoding="utf-8"))

        self.assertEqual(rendered[0], rendered[1])
        root = ET.fromstring(rendered[0])
        self.assertEqual(root.tag.rsplit("}", 1)[-1], "svg")
        self.assertEqual(root.attrib["width"], "640.0")
        self.assertEqual(root.attrib["height"], "480.0")
        self.assertNotIn("x-axis", rendered[0])
        self.assertNotIn("hidden-lines", rendered[0])

    def test_cli_suppresses_source_output(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "model.py"
            glb, svg, facts = self.render_paths(root)
            source.write_text(
                "import cadquery as cq\nprint('do not leak')\n"
                "result = cq.Workplane('XY').box(1, 1, 1)\n",
                encoding="utf-8",
            )

            completed = subprocess.run(
                [
                    sys.executable,
                    "-m",
                    "renderer",
                    str(source),
                    str(glb),
                    str(svg),
                    str(facts),
                ],
                check=False,
                capture_output=True,
                text=True,
            )

            self.assertEqual(completed.returncode, 0, completed.stderr)
            self.assertEqual(completed.stdout, "")
            self.assertEqual(completed.stderr, "")
            self.assertTrue(all(path.exists() for path in (glb, svg, facts)))

    def test_cli_reports_only_a_stable_error_category(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "model.py"
            outputs = self.render_paths(root)
            source.write_text(
                "print('source text')\nraise RuntimeError('secret details')\n",
                encoding="utf-8",
            )

            completed = subprocess.run(
                [sys.executable, "-m", "renderer", str(source), *(str(path) for path in outputs)],
                check=False,
                capture_output=True,
                text=True,
            )

            self.assertEqual(completed.returncode, 1)
            self.assertEqual(completed.stdout, "")
            self.assertEqual(
                completed.stderr,
                "renderer_error=source_execution_failed\n",
            )
            self.assertFalse(any(path.exists() for path in outputs))

    def test_cli_rejects_wrong_argument_count(self) -> None:
        completed = subprocess.run(
            [sys.executable, "-m", "renderer", "source.py", "model.glb"],
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertEqual(completed.returncode, 2)
        self.assertEqual(completed.stdout, "")
        self.assertEqual(completed.stderr, "renderer_error=invalid_arguments\n")

    def test_source_failures_have_stable_categories(self) -> None:
        cases = {
            "missing_result": "value = 1\n",
            "unsupported_result": "result = object()\n",
            "source_execution_failed": "raise RuntimeError('secret details')\n",
        }
        for expected, source_text in cases.items():
            with self.subTest(expected=expected), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                source = root / "model.py"
                source.write_text(source_text, encoding="utf-8")
                self.assertEqual(render(source, *self.render_paths(root)), expected)

    def test_invalid_utf8_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "model.py"
            source.write_bytes(b"result = '\xff'\n")
            self.assertEqual(render(source, *self.render_paths(root)), "invalid_utf8")

    def test_invalid_paths_are_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "model.py"
            source.write_text("result = None\n", encoding="utf-8")
            existing_output = root / "existing.glb"
            existing_output.write_bytes(b"stale")
            glb, svg, facts = self.render_paths(root)
            dangling_output = root / "dangling.svg"
            dangling_output.symlink_to(root / "missing.svg")

            self.assertEqual(
                render(root / "missing.py", glb, svg, facts),
                "invalid_source_path",
            )
            self.assertEqual(render(source, source, svg, facts), "invalid_output_path")
            self.assertEqual(render(source, existing_output, svg, facts), "invalid_output_path")
            self.assertEqual(
                render(source, root / "missing" / "model.glb", svg, facts),
                "invalid_output_path",
            )
            self.assertEqual(render(source, glb, glb, facts), "invalid_output_path")
            self.assertEqual(render(source, glb, svg, svg), "invalid_output_path")
            self.assertEqual(
                render(source, glb, dangling_output, facts),
                "invalid_output_path",
            )
            self.assertEqual(existing_output.read_bytes(), b"stale")

    def test_malformed_generated_outputs_are_removed(self) -> None:
        cases = {
            "invalid_facts": mock.patch("renderer.worker._validate_facts", return_value=False),
            "invalid_svg": mock.patch("renderer.worker._validate_svg", return_value=False),
            "invalid_glb": mock.patch("renderer.worker._validate_glb", return_value=False),
        }
        for expected, patcher in cases.items():
            with self.subTest(expected=expected), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                source = root / "model.py"
                source.write_text(
                    "import cadquery as cq\nresult = cq.Workplane('XY').box(1, 1, 1)\n",
                    encoding="utf-8",
                )
                outputs = self.render_paths(root)
                with patcher:
                    self.assertEqual(render(source, *outputs), expected)
                self.assertFalse(any(path.exists() for path in outputs))

    def test_facts_validator_rejects_nonfinite_and_extra_values(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            facts_path = Path(directory) / "model.json"
            invalid_facts = (
                '{"volume_cubic_millimeters":NaN,"size_millimeters":{"x":1,"y":2,"z":3}}',
                '{"volume_cubic_millimeters":1,"size_millimeters":{"x":1,"y":2,"z":-1}}',
                '{"volume_cubic_millimeters":1,"size_millimeters":{"x":1,"y":2,"z":3},"extra":0}',
            )
            for content in invalid_facts:
                facts_path.write_text(content, encoding="utf-8")
                self.assertFalse(_validate_facts(facts_path))

    def test_glb_validator_accepts_json_only_and_json_with_bin(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "model.glb"
            for content in (
                make_glb(b'{"asset":{"version":"2.0"}} '),
                make_glb(b'{"asset":{"version":"2.0"}} ', b"bin\x00"),
            ):
                output.write_bytes(content)
                self.assertTrue(_validate_glb(output))

    def test_glb_validator_rejects_malformed_containers(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "model.glb"
            valid_json = b'{"asset":{"version":"2.0"}} '
            cases = {
                "short header": b"not glb",
                "wrong declared size": struct.pack(
                    "<4sIIII", b"glTF", 2, 999, 0, 0x4E4F534A
                ),
                "zero-length JSON": make_glb(b""),
                "invalid JSON": make_glb(b"not-json"),
                "non-object JSON": make_glb(b"[1] "),
                "nonstandard JSON constant": make_glb(b"NaN "),
                "NUL-padded JSON": make_glb(b"{} \x00"),
                "truncated JSON": set_declared_glb_size(make_glb(valid_json)[:-1]),
                "wrong first chunk type": make_glb(valid_json).replace(
                    struct.pack("<I", 0x4E4F534A),
                    struct.pack("<I", 0x004E4942),
                    1,
                ),
                "truncated BIN": set_declared_glb_size(
                    make_glb(valid_json, b"mesh")[:-1]
                ),
                "wrong second chunk type": make_glb(valid_json, b"mesh").replace(
                    struct.pack("<I", 0x004E4942),
                    struct.pack("<I", 0x4E4F534A),
                    1,
                ),
                "trailing bytes": set_declared_glb_size(
                    make_glb(valid_json) + b"bad!"
                ),
            }
            for name, content in cases.items():
                with self.subTest(name=name):
                    output.write_bytes(content)
                    self.assertFalse(_validate_glb(output))


if __name__ == "__main__":
    unittest.main()
