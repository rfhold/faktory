from __future__ import annotations

import contextlib
import json
import math
import os
import struct
import sys
import xml.etree.ElementTree as ET
from pathlib import Path
from typing import Final

import cadquery as cq
from cadquery.occ_impl.exporters import getSVG
from cadquery.occ_impl.exporters.assembly import exportGLTF

ERROR_PREFIX: Final = "renderer_error="
PROJECTIONS: Final = (
    ("isometric", (1, -1, 1)),
    ("front", (0, -1, 0)),
    ("back", (0, 1, 0)),
    ("left", (-1, 0, 0)),
    ("right", (1, 0, 0)),
    ("top", (0, 0, 1)),
    ("bottom", (0, 0, -1)),
)


def _reject_json_constant(value: str) -> None:
    raise ValueError(f"invalid JSON constant: {value}")


def _validate_paths(source_path: Path, output_paths: tuple[Path, ...]) -> str | None:
    try:
        if not source_path.is_file():
            return "invalid_source_path"
        resolved_paths = [source_path.resolve(), *(path.resolve() for path in output_paths)]
        if len(set(resolved_paths)) != len(resolved_paths):
            return "invalid_output_path"
        for output_path in output_paths:
            if (
                not output_path.parent.is_dir()
                or output_path.exists()
                or output_path.is_symlink()
            ):
                return "invalid_output_path"
    except OSError:
        return "invalid_path"
    return None


def _validate_glb(output_path: Path) -> bool:
    try:
        content = output_path.read_bytes()
        if len(content) < 20:
            return False
        magic, version, declared_size = struct.unpack_from("<4sII", content)
    except (OSError, struct.error):
        return False

    if magic != b"glTF" or version != 2 or declared_size != len(content):
        return False

    offset = 12
    try:
        json_size, json_type = struct.unpack_from("<II", content, offset)
    except struct.error:
        return False
    offset += 8
    if json_size == 0 or json_size % 4 != 0 or json_type != 0x4E4F534A:
        return False
    json_end = offset + json_size
    if json_end > len(content):
        return False
    try:
        document = json.loads(
            content[offset:json_end].decode("utf-8"),
            parse_constant=_reject_json_constant,
        )
    except (UnicodeDecodeError, ValueError, RecursionError):
        return False
    if not isinstance(document, dict):
        return False
    offset = json_end

    if offset < len(content):
        if len(content) - offset < 8:
            return False
        try:
            bin_size, bin_type = struct.unpack_from("<II", content, offset)
        except struct.error:
            return False
        offset += 8
        if bin_size % 4 != 0 or bin_type != 0x004E4942:
            return False
        offset += bin_size

    return offset == len(content)


def _validate_svg(output_path: Path) -> bool:
    try:
        content = output_path.read_text(encoding="utf-8")
        root = ET.fromstring(content)
    except (OSError, UnicodeDecodeError, ET.ParseError):
        return False
    return bool(content.strip()) and root.tag.rsplit("}", 1)[-1] == "svg"


def _valid_number(value: object) -> bool:
    return (
        isinstance(value, (int, float))
        and not isinstance(value, bool)
        and math.isfinite(value)
        and value >= 0
    )


def _validate_facts(output_path: Path) -> bool:
    try:
        facts = json.loads(output_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError):
        return False
    if not isinstance(facts, dict) or set(facts) != {
        "volume_cubic_millimeters",
        "size_millimeters",
    }:
        return False
    size = facts["size_millimeters"]
    return (
        _valid_number(facts["volume_cubic_millimeters"])
        and isinstance(size, dict)
        and set(size) == {"x", "y", "z"}
        and all(_valid_number(size[axis]) for axis in ("x", "y", "z"))
    )


def _remove_outputs(output_paths: tuple[Path, ...]) -> None:
    for output_path in output_paths:
        try:
            output_path.unlink(missing_ok=True)
        except OSError:
            pass


def render(
    source_path: Path,
    glb_path: Path,
    svg_path: Path,
    facts_path: Path,
    projection_paths: tuple[Path, ...],
) -> str | None:
    if len(projection_paths) != len(PROJECTIONS):
        return "invalid_arguments"
    output_paths = (glb_path, svg_path, facts_path, *projection_paths)
    path_error = _validate_paths(source_path, output_paths)
    if path_error is not None:
        return path_error

    try:
        source = source_path.read_text(encoding="utf-8")
    except UnicodeDecodeError:
        _remove_outputs(output_paths)
        return "invalid_utf8"
    except OSError:
        _remove_outputs(output_paths)
        return "source_read_failed"

    namespace = {
        "__file__": str(source_path),
        "__name__": "__main__",
        "__package__": None,
    }
    try:
        code = compile(source, str(source_path), "exec")
        with open(os.devnull, "w", encoding="utf-8") as devnull:
            with contextlib.redirect_stdout(devnull), contextlib.redirect_stderr(devnull):
                exec(code, namespace)
    except BaseException:
        _remove_outputs(output_paths)
        return "source_execution_failed"

    if "result" not in namespace:
        _remove_outputs(output_paths)
        return "missing_result"

    result = namespace["result"]
    if isinstance(result, cq.Assembly):
        assembly = result
    elif isinstance(result, (cq.Workplane, cq.Shape)):
        try:
            assembly = cq.Assembly(result)
        except Exception:
            _remove_outputs(output_paths)
            return "unsupported_result"
    else:
        _remove_outputs(output_paths)
        return "unsupported_result"

    try:
        compound = assembly.toCompound()
        bounding_box = compound.BoundingBox()
        facts = {
            "volume_cubic_millimeters": compound.Volume(),
            "size_millimeters": {
                "x": bounding_box.xlen,
                "y": bounding_box.ylen,
                "z": bounding_box.zlen,
            },
        }
        if not _valid_number(facts["volume_cubic_millimeters"]) or not all(
            _valid_number(facts["size_millimeters"][axis])
            for axis in ("x", "y", "z")
        ):
            raise ValueError("invalid geometry facts")
    except BaseException:
        _remove_outputs(output_paths)
        return "geometry_failed"

    try:
        facts_path.write_text(
            json.dumps(facts, allow_nan=False, separators=(",", ":")) + "\n",
            encoding="utf-8",
        )
    except BaseException:
        _remove_outputs(output_paths)
        return "facts_export_failed"
    if not _validate_facts(facts_path):
        _remove_outputs(output_paths)
        return "invalid_facts"

    for (name, direction), projection_path in zip(
        PROJECTIONS, projection_paths, strict=True
    ):
        try:
            svg = getSVG(
                compound,
                {
                    "width": 640,
                    "height": 480,
                    "marginLeft": 32,
                    "marginTop": 32,
                    "projectionDir": direction,
                    "showAxes": False,
                    "showHidden": False,
                    "strokeColor": (24, 24, 24),
                },
            )
            projection_path.write_text(svg, encoding="utf-8")
            if name == "isometric":
                svg_path.write_text(svg, encoding="utf-8")
        except BaseException:
            _remove_outputs(output_paths)
            return "svg_export_failed"
        if not _validate_svg(projection_path):
            _remove_outputs(output_paths)
            return "invalid_svg"
    if not _validate_svg(svg_path):
        _remove_outputs(output_paths)
        return "invalid_svg"

    try:
        with open(os.devnull, "w", encoding="utf-8") as devnull:
            with contextlib.redirect_stdout(devnull), contextlib.redirect_stderr(devnull):
                exported = exportGLTF(assembly, str(glb_path), binary=True)
        if exported is False:
            raise RuntimeError("CadQuery reported an unsuccessful export")
    except BaseException:
        _remove_outputs(output_paths)
        return "export_failed"

    if not _validate_glb(glb_path):
        _remove_outputs(output_paths)
        return "invalid_glb"
    return None


def main(argv: list[str] | None = None) -> int:
    args = sys.argv[1:] if argv is None else argv
    if len(args) != 11:
        print(f"{ERROR_PREFIX}invalid_arguments", file=sys.stderr)
        return 2

    error = render(
        Path(args[0]),
        Path(args[1]),
        Path(args[2]),
        Path(args[3]),
        tuple(Path(path) for path in args[4:]),
    )
    if error is not None:
        print(f"{ERROR_PREFIX}{error}", file=sys.stderr)
        return 1
    return 0
