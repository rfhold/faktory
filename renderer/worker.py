from __future__ import annotations

import ast
import contextlib
import json
import math
import os
import shutil
import stat
import struct
import sys
import sysconfig
import types
import uuid
import xml.etree.ElementTree as ET
from collections.abc import Iterator
from pathlib import Path
from typing import Final

import cadquery as cq
from cadquery.occ_impl.exporters import svg as cq_svg
from cadquery.occ_impl.exporters.assembly import exportGLTF
from OCP.BRepLib import BRepLib
from OCP.HLRAlgo import HLRAlgo_Projector
from OCP.HLRBRep import HLRBRep_Algo, HLRBRep_HLRToShape
from OCP.gp import gp_Ax2, gp_Dir, gp_Pnt
from renderer import faktory_design as _design_package
from renderer.faktory_design import v1 as _design_v1
from renderer.faktory_design.v1 import Design, Output

sys.modules.setdefault("faktory_design", _design_package)
sys.modules.setdefault("faktory_design.v1", _design_v1)

ERROR_PREFIX: Final = "renderer_error="
MAX_GLB_BYTES: Final = 64 * 1024 * 1024
MAX_BUNDLE_BYTES: Final = 192 * 1024 * 1024
PROJECTIONS: Final = (
    ("isometric", (1, -1, 1), (1, 1, 0)),
    ("front", (0, -1, 0), (1, 0, 0)),
    ("back", (0, 1, 0), (-1, 0, 0)),
    ("left", (-1, 0, 0), (0, -1, 0)),
    ("right", (1, 0, 0), (0, 1, 0)),
    ("top", (0, 0, 1), (1, 0, 0)),
    ("bottom", (0, 0, -1), (1, 0, 0)),
)


def _get_svg(
    shape: cq.Shape,
    projection_direction: tuple[int, int, int],
    screen_right: tuple[int, int, int],
) -> str:
    # CadQuery 2.6.1's getSVG does not expose gp_Ax2's X direction. Keep this
    # bounded adapter aligned with its HLR and SVG helpers while fixing the roll.
    hlr = HLRBRep_Algo()
    hlr.Add(shape.wrapped)
    coordinate_system = gp_Ax2(
        gp_Pnt(), gp_Dir(*projection_direction), gp_Dir(*screen_right)
    )
    hlr.Projector(HLRAlgo_Projector(coordinate_system))
    hlr.Update()
    hlr.Hide()

    hlr_shapes = HLRBRep_HLRToShape(hlr)
    visible = [
        edge_set
        for edge_set in (
            hlr_shapes.VCompound(),
            hlr_shapes.Rg1LineVCompound(),
            hlr_shapes.OutLineVCompound(),
        )
        if not edge_set.IsNull()
    ]
    hidden = [
        edge_set
        for edge_set in (hlr_shapes.HCompound(), hlr_shapes.OutLineHCompound())
        if not edge_set.IsNull()
    ]

    for edge_set in (*visible, *hidden):
        BRepLib.BuildCurves3d_s(edge_set, cq_svg.TOLERANCE)

    visible_shapes = list(map(cq_svg.Shape, visible))
    hidden_shapes = list(map(cq_svg.Shape, hidden))
    _, visible_paths = cq_svg.getPaths(visible_shapes, hidden_shapes)
    bounding_box = cq_svg.Compound.makeCompound(
        hidden_shapes + visible_shapes
    ).BoundingBox()

    width = 640.0
    height = 480.0
    margin_left = 32.0
    margin_top = 32.0
    bounding_box_scale = 0.75
    unit_scale = min(
        width / bounding_box.xlen * bounding_box_scale,
        height / bounding_box.ylen * bounding_box_scale,
    )
    x_translate = -bounding_box.xmin + margin_left / unit_scale
    y_translate = -bounding_box.ymax - margin_top / unit_scale
    stroke_width = 1.0 / unit_scale
    visible_content = "".join(cq_svg.PATHTEMPLATE % path for path in visible_paths)

    return cq_svg.SVG_TEMPLATE % {
        "unitScale": str(unit_scale),
        "strokeWidth": str(stroke_width),
        "strokeColor": "24,24,24",
        "hiddenColor": "160,160,160",
        "hiddenContent": "",
        "visibleContent": visible_content,
        "xTranslate": str(x_translate),
        "yTranslate": str(y_translate),
        "width": str(width),
        "height": str(height),
        "axesIndicator": "",
    }


def _reject_json_constant(value: str) -> None:
    raise ValueError(f"invalid JSON constant: {value}")


def _is_relative_to(path: Path, root: Path) -> bool:
    try:
        path.relative_to(root)
    except ValueError:
        return False
    return True


def _validate_paths(
    project_root: Path,
    entrypoint: Path,
    library_root: Path,
    output_root: Path,
) -> tuple[str | None, Path | None, Path | None, Path | None, Path | None]:
    try:
        if not project_root.is_dir() or project_root.is_symlink():
            return "invalid_project_root", None, None, None, None
        if not library_root.is_dir() or library_root.is_symlink():
            return "invalid_library_root", None, None, None, None
        if entrypoint.is_absolute() or ".." in entrypoint.parts:
            return "invalid_entrypoint", None, None, None, None
        resolved_project_root = project_root.resolve(strict=True)
        resolved_library_root = library_root.resolve(strict=True)
        lexical_entrypoint = resolved_project_root / entrypoint
        current = resolved_project_root
        for part in entrypoint.parts:
            current /= part
            if current.is_symlink():
                return "invalid_entrypoint", None, None, None, None
        if not lexical_entrypoint.is_file():
            return "invalid_entrypoint", None, None, None, None
        resolved_entrypoint = lexical_entrypoint.resolve(strict=True)
        if (
            not _is_relative_to(resolved_entrypoint, resolved_project_root)
            or not resolved_entrypoint.is_file()
            or resolved_entrypoint.suffix != ".py"
        ):
            return "invalid_entrypoint", None, None, None, None
        if (
            not output_root.name
            or not output_root.parent.is_dir()
            or output_root.parent.is_symlink()
            or output_root.parent.resolve(strict=True) != output_root.parent.absolute()
            or output_root.exists()
            or output_root.is_symlink()
        ):
            return "invalid_output_path", None, None, None, None
        resolved_output_root = output_root.resolve()
        resolved_paths = [
            resolved_entrypoint,
            resolved_project_root,
            resolved_library_root,
            resolved_output_root,
        ]
        if len(set(resolved_paths)) != len(resolved_paths):
            return "invalid_output_path", None, None, None, None
    except (OSError, RuntimeError):
        return "invalid_path", None, None, None, None
    return (
        None,
        resolved_project_root,
        resolved_entrypoint,
        resolved_library_root,
        resolved_output_root,
    )


def _interpreter_paths(original: list[str]) -> list[str]:
    prefixes = {Path(sys.prefix).resolve(), Path(sys.base_prefix).resolve()}
    configured = {
        Path(path).resolve()
        for path in sysconfig.get_paths().values()
        if path
    }
    retained: list[str] = []
    for item in original:
        if not item:
            continue
        try:
            candidate = Path(item).resolve()
        except (OSError, RuntimeError):
            continue
        if candidate in configured or any(_is_relative_to(candidate, root) for root in prefixes):
            value = str(candidate)
            if value not in retained:
                retained.append(value)
    return retained


def _shared_target_allowed(target: str, own_namespace: str) -> bool:
    return target == own_namespace or target.startswith(f"{own_namespace}.")


def _from_import_allowed(
    target: str, names: list[ast.alias], own_namespace: str
) -> bool:
    if target == "faktory_shared":
        return all(
            name.name != "*"
            and _shared_target_allowed(f"{target}.{name.name}", own_namespace)
            for name in names
        )
    return _shared_target_allowed(target, own_namespace)


def _library_imports_allowed(
    tree: ast.AST, package: str, own_namespace: str
) -> bool:
    package_parts = package.split(".")
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for name in node.names:
                if name.name == "faktory_shared" or name.name.startswith(
                    "faktory_shared."
                ):
                    if not _shared_target_allowed(name.name, own_namespace):
                        return False
        elif isinstance(node, ast.ImportFrom):
            if node.level:
                parent_count = node.level - 1
                if parent_count >= len(package_parts):
                    return False
                base = package_parts[: len(package_parts) - parent_count]
                target = ".".join((*base, *(node.module or "").split("."))).rstrip(".")
                if not _from_import_allowed(target, node.names, own_namespace):
                    return False
            elif node.module == "faktory_shared" or (
                node.module is not None
                and node.module.startswith("faktory_shared.")
            ):
                if not _from_import_allowed(node.module, node.names, own_namespace):
                    return False
    return True


def _validate_library_sources(library_root: Path) -> str | None:
    shared_root = library_root / "faktory_shared"
    if not shared_root.exists():
        return None
    try:
        for source_path in sorted(shared_root.rglob("*.py")):
            relative = source_path.relative_to(library_root)
            if source_path.is_symlink() or len(relative.parts) < 3:
                if relative != Path("faktory_shared/__init__.py"):
                    return "invalid_library_source"
                own_namespace = "faktory_shared"
            else:
                own_namespace = f"faktory_shared.{relative.parts[1]}"
            resolved_source = source_path.resolve(strict=True)
            if not _is_relative_to(resolved_source, library_root):
                return "invalid_library_source"
            source = source_path.read_text(encoding="utf-8")
            tree = ast.parse(source, filename=str(source_path))
            package_parts = relative.with_suffix("").parts[:-1]
            package = ".".join(package_parts)
            if not _library_imports_allowed(tree, package, own_namespace):
                return "invalid_library_source"
    except (OSError, RuntimeError, SyntaxError, UnicodeError):
        return "invalid_library_source"
    return None


@contextlib.contextmanager
def _execution_environment(
    project_root: Path, library_root: Path, entrypoint: Path
) -> Iterator[dict[str, object]]:
    original_cwd = Path.cwd()
    original_path = sys.path.copy()
    original_modules = sys.modules.copy()
    original_importer_cache = sys.path_importer_cache.copy()
    package_parts = entrypoint.relative_to(project_root).parent.parts
    package = ".".join(package_parts)
    namespace: dict[str, object] = {
        "__file__": str(entrypoint),
        "__name__": "__main__",
        "__package__": package,
        "__spec__": None,
    }
    main_module = types.ModuleType("__main__")
    main_module.__dict__.update(namespace)
    project_names = {
        path.stem if path.is_file() else path.name
        for path in project_root.iterdir()
        if (path.is_file() and path.suffix == ".py") or path.is_dir()
    }
    try:
        os.chdir(project_root)
        sys.path[:] = [
            str(library_root),
            str(project_root),
            *_interpreter_paths(original_path),
        ]
        for name in tuple(sys.modules):
            top_level = name.partition(".")[0]
            if top_level == "faktory_shared" or (
                top_level in project_names and top_level != "faktory_design"
            ):
                del sys.modules[name]
        sys.modules["__main__"] = main_module
        yield main_module.__dict__
    finally:
        os.chdir(original_cwd)
        sys.path[:] = original_path
        sys.modules.clear()
        sys.modules.update(original_modules)
        sys.path_importer_cache.clear()
        sys.path_importer_cache.update(original_importer_cache)


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


def _remove_output_tree(output_root: Path) -> None:
    try:
        shutil.rmtree(output_root)
    except OSError:
        pass


def _regular_file_size(path: Path) -> int | None:
    try:
        metadata = path.stat(follow_symlinks=False)
    except OSError:
        return None
    if not stat.S_ISREG(metadata.st_mode):
        return None
    return metadata.st_size


def _regular_file_within_limit(path: Path, max_bytes: int) -> bool | None:
    size = _regular_file_size(path)
    return None if size is None else size <= max_bytes


def _expected_bundle_paths(output_root: Path, outputs: tuple[Output, ...]) -> Iterator[Path]:
    yield output_root / "outputs.json"
    for output in outputs:
        artifact_root = output_root / "outputs" / output.output_id
        yield artifact_root / "model.glb"
        yield artifact_root / "preview.svg"
        yield artifact_root / "facts.json"
        for name, _, _ in PROJECTIONS:
            yield artifact_root / "projections" / f"{name}.svg"


def _bundle_within_limit(
    output_root: Path,
    outputs: tuple[Output, ...],
    max_bytes: int = MAX_BUNDLE_BYTES,
) -> bool:
    total = 0
    for path in _expected_bundle_paths(output_root, outputs):
        size = _regular_file_size(path)
        if size is None or size < 0 or size > max_bytes - total:
            return False
        total += size
    return True


def _stabilize_assembly_names(assembly: cq.Assembly) -> None:
    def visit(current: cq.Assembly, stable_name: str) -> None:
        try:
            uuid.UUID(current.name)
        except (ValueError, AttributeError):
            pass
        else:
            current.name = stable_name
        for index, child in enumerate(current.children):
            visit(child, f"{stable_name}_{index}")

    visit(assembly, "result")


def _normalize_result(result: object) -> tuple[str | None, tuple[Output, ...] | None]:
    if type(result) is Design:
        try:
            outputs = tuple(
                Output(
                    output_id=output.output_id,
                    role=output.role,
                    geometry=output.geometry,
                    primary=output.primary,
                )
                for output in result.outputs
                if type(output) is Output
            )
            if len(outputs) != len(result.outputs):
                return "invalid_design", None
            normalized = Design(outputs)
        except BaseException:
            return "invalid_design", None
        return None, normalized.outputs
    if isinstance(result, cq.Assembly):
        role = "assembly"
    elif isinstance(result, (cq.Workplane, cq.Shape)):
        role = "part"
    else:
        return "unsupported_result", None
    return None, (Output("primary", role, result, primary=True),)


def _render_output(
    output: Output, output_root: Path
) -> tuple[str | None, dict[str, object] | None]:
    if isinstance(output.geometry, cq.Assembly):
        assembly = output.geometry
    elif isinstance(output.geometry, (cq.Workplane, cq.Shape)):
        try:
            assembly = cq.Assembly(output.geometry, name="result")
        except Exception:
            return "unsupported_geometry", None
    else:
        return "unsupported_geometry", None
    _stabilize_assembly_names(assembly)

    try:
        compound = assembly.toCompound()
        bounding_box = compound.BoundingBox()
        facts: dict[str, object] = {
            "volume_cubic_millimeters": compound.Volume(),
            "size_millimeters": {
                "x": bounding_box.xlen,
                "y": bounding_box.ylen,
                "z": bounding_box.zlen,
            },
        }
        size = facts["size_millimeters"]
        if not _valid_number(facts["volume_cubic_millimeters"]) or not isinstance(
            size, dict
        ) or not all(_valid_number(size[axis]) for axis in ("x", "y", "z")):
            raise ValueError("invalid geometry facts")
    except BaseException:
        return "geometry_failed", None

    projection_root = output_root / "projections"
    try:
        projection_root.mkdir(parents=True)
        facts_path = output_root / "facts.json"
        facts_path.write_text(
            json.dumps(facts, allow_nan=False, separators=(",", ":")) + "\n",
            encoding="utf-8",
        )
    except BaseException:
        return "facts_export_failed", None
    if not _validate_facts(facts_path):
        return "invalid_facts", None

    preview_path = output_root / "preview.svg"
    for name, direction, screen_right in PROJECTIONS:
        projection_path = projection_root / f"{name}.svg"
        try:
            svg = _get_svg(compound, direction, screen_right)
            projection_path.write_text(svg, encoding="utf-8")
            if name == "isometric":
                preview_path.write_text(svg, encoding="utf-8")
        except BaseException:
            return "svg_export_failed", None
        if not _validate_svg(projection_path):
            return "invalid_svg", None
    if not _validate_svg(preview_path):
        return "invalid_svg", None

    glb_path = output_root / "model.glb"
    try:
        with open(os.devnull, "w", encoding="utf-8") as devnull:
            with contextlib.redirect_stdout(devnull), contextlib.redirect_stderr(devnull):
                exported = exportGLTF(assembly, str(glb_path), binary=True)
        if exported is False:
            raise RuntimeError("CadQuery reported an unsuccessful export")
    except BaseException:
        return "export_failed", None
    glb_within_limit = _regular_file_within_limit(glb_path, MAX_GLB_BYTES)
    if glb_within_limit is None:
        return "invalid_glb", None
    if not glb_within_limit:
        return "glb_too_large", None
    if not _validate_glb(glb_path):
        return "invalid_glb", None
    return None, facts


def render(
    project_root: Path,
    entrypoint: Path,
    library_root: Path,
    output_root: Path,
) -> str | None:
    path_error, project_root, source_path, library_root, output_root = _validate_paths(
        project_root, entrypoint, library_root, output_root
    )
    if path_error is not None:
        return path_error
    assert project_root is not None
    assert source_path is not None
    assert library_root is not None
    assert output_root is not None

    temporary_root = output_root.with_name(f".{output_root.name}.{uuid.uuid4().hex}.tmp")
    try:
        temporary_root.mkdir(mode=0o700)
    except OSError:
        return "output_create_failed"

    library_error = _validate_library_sources(library_root)
    if library_error is not None:
        _remove_output_tree(temporary_root)
        return library_error

    try:
        source = source_path.read_text(encoding="utf-8")
    except UnicodeDecodeError:
        _remove_output_tree(temporary_root)
        return "invalid_utf8"
    except OSError:
        _remove_output_tree(temporary_root)
        return "source_read_failed"

    try:
        code = compile(source, str(source_path), "exec")
        with open(os.devnull, "w", encoding="utf-8") as devnull:
            with contextlib.redirect_stdout(devnull), contextlib.redirect_stderr(devnull):
                with _execution_environment(
                    project_root, library_root, source_path
                ) as namespace:
                    exec(code, namespace)
    except BaseException:
        _remove_output_tree(temporary_root)
        return "source_execution_failed"

    if "result" not in namespace:
        _remove_output_tree(temporary_root)
        return "missing_result"

    result_error, outputs = _normalize_result(namespace["result"])
    if result_error is not None or outputs is None:
        _remove_output_tree(temporary_root)
        return result_error

    summaries = []
    for output in outputs:
        output_path = temporary_root / "outputs" / output.output_id
        output_error, facts = _render_output(output, output_path)
        if output_error is not None or facts is None:
            _remove_output_tree(temporary_root)
            return output_error
        summaries.append(
            {
                "output_id": output.output_id,
                "role": output.role,
                "primary": output.primary,
                "facts": facts,
            }
        )

    manifest = {"format": "faktory-outputs-v1", "outputs": summaries}
    try:
        (temporary_root / "outputs.json").write_text(
            json.dumps(manifest, allow_nan=False, separators=(",", ":")) + "\n",
            encoding="utf-8",
        )
    except BaseException:
        _remove_output_tree(temporary_root)
        return "manifest_export_failed"
    if not _bundle_within_limit(temporary_root, outputs):
        _remove_output_tree(temporary_root)
        return "bundle_too_large"
    try:
        temporary_root.rename(output_root)
    except OSError:
        _remove_output_tree(temporary_root)
        return "output_publish_failed"
    return None


def main(argv: list[str] | None = None) -> int:
    args = sys.argv[1:] if argv is None else argv
    if len(args) != 4:
        print(f"{ERROR_PREFIX}invalid_arguments", file=sys.stderr)
        return 2

    error = render(
        Path(args[0]),
        Path(args[1]),
        Path(args[2]),
        Path(args[3]),
    )
    if error is not None:
        print(f"{ERROR_PREFIX}{error}", file=sys.stderr)
        return 1
    return 0
