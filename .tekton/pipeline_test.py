import json
import os
import re
import subprocess
import tempfile
import textwrap
import unittest
from pathlib import Path


ROOT = Path(__file__).parent


class VisualRendererPipelineTest(unittest.TestCase):
    def setUp(self) -> None:
        self.preview = (ROOT / "faktory-preview.yaml").read_text()
        self.release = (ROOT / "faktory-release.yaml").read_text()
        self.verifier = (ROOT / "verify-visual-renderer.mjs").read_text()

    def test_worker_build_is_amd64_only(self) -> None:
        for pipeline in (self.preview,):
            self.assertEqual(pipeline.count("target=visual-renderer-runtime"), 1)
            self.assertIn("target=visual-renderer-runtime --opt platform=linux/amd64", pipeline)
            unsupported_build = "target=visual-renderer-runtime --opt platform=linux/" + "arm64"
            self.assertNotIn(unsupported_build, pipeline)
            self.assertIn("/faktory-visual-renderer", pipeline)

    def test_renderer_smoke_exercises_complete_multipart_worker_contract(self) -> None:
        for pipeline in (self.preview,):
            self.assertEqual(pipeline.count('dependency_root="$output_dir/dependencies"'), 2)
            self.assertEqual(pipeline.count('"format":"faktory-model-dependencies-v1"'), 2)
            self.assertEqual(pipeline.count('"root_model_id":"pipeline-smoke"'), 2)
            self.assertEqual(pipeline.count('"package":"faktory_models.m_pipeline_smoke"'), 2)
            self.assertEqual(pipeline.count('> "$dependency_root/dependencies.json"'), 2)
            self.assertNotIn('library_root="$output_dir/libraries"', pipeline)
            self.assertEqual(pipeline.count('Output("secondary", "part"'), 2)
            self.assertEqual(pipeline.count('bundle / "outputs.json"'), 2)
            self.assertEqual(pipeline.count('root / "model.glb"'), 4)
            self.assertEqual(pipeline.count('root / "preview.svg"'), 2)
            self.assertEqual(pipeline.count('root / "facts.json"'), 2)
            self.assertEqual(pipeline.count('root / "projections"'), 2)
            self.assertEqual(pipeline.count('bundle.rglob("*.png")'), 2)
            self.assertEqual(pipeline.count('path.name == "renders"'), 2)
            self.assertNotIn("renderer/examples/box.py", pipeline)

    def test_native_verification_is_hardened_and_blocks_deployment(self) -> None:
        for pipeline in (self.preview,):
            task_run = re.search(
                r"pipelineTaskName: verify-visual-renderer-amd64(?P<body>[\s\S]*?)"
                r"pipelineTaskName: (?:deploy-preview|deploy-production)",
                pipeline,
            )
            self.assertIsNotNone(task_run)
            body = task_run.group("body")
            self.assertIn("kubernetes.io/arch: amd64", body)
            self.assertIn("runAsUser: 65532", body)
            self.assertIn("seccompProfile:\n            type: Unconfined", body)
            self.assertIn("medium: Memory", body)
            self.assertIn("sizeLimit: 256Mi", body)
            self.assertIn("allowPrivilegeEscalation: false", pipeline)
            self.assertIn("readOnlyRootFilesystem: true", pipeline)
            self.assertIn('capabilities: { drop: ["ALL"] }', pipeline)
            self.assertIn("node \"$(workspaces.source.path)/.tekton/verify-visual-renderer.mjs\"", pipeline)
            self.assertIn("visualRendererImage=$(params.visual-renderer-image)", pipeline)
        forbidden_argument = "--no-" + "sandbox"
        self.assertNotIn(forbidden_argument, self.preview + self.release + self.verifier)
        self.assertIn("runAfter: [resolve-image, resolve-visual-renderer-image]", self.preview)
        self.assertIn("runAfter: [promote-release]", self.release)

    def test_native_verifier_renders_colored_canonical_response(self) -> None:
        self.assertIn('recipe: "three-v2"', self.verifier)
        self.assertIn('baseColorFactor: [1, 0.02, 0.02, 1]', self.verifier)
        self.assertIn('const names = ["isometric", "front", "back", "left", "right", "top", "bottom"]', self.verifier)
        self.assertIn('image.mime_type !== "image/png"', self.verifier)
        self.assertIn("png.width !== 640 || png.height !== 480", self.verifier)
        self.assertIn("redPixels < 100", self.verifier)

    def test_release_promotes_an_immutable_worker_digest(self) -> None:
        self.assertIn('$worker:preview-$(params.revision)-amd64', self.release)
        self.assertIn('worker_image="$worker@$worker_digest"', self.release)
        self.assertIn('.config.User == "65532:65532"', self.release)
        self.assertIn('.config.Labels["org.opencontainers.image.revision"] == $revision', self.release)


class ReleasePromotionTest(unittest.TestCase):
    revision = "a" * 40
    app_digest = "sha256:" + "1" * 64
    worker_digest = "sha256:" + "2" * 64

    def setUp(self) -> None:
        self.release = (ROOT / "faktory-release.yaml").read_text()
        task = self.release.split("      - name: promote-release\n", 1)[1]
        step = task.split("            - name: verify-and-promote\n", 1)[1]
        script = step.split("              script: |\n", 1)[1].split("\n        params:", 1)[0]
        self.script = textwrap.dedent(script)

    def test_release_graph_preserves_gates_without_building(self) -> None:
        self.assertIn("runAfter: [validate-release, scan-private-material]", self.release)
        self.assertIn("secretName: faktory-release-trusted-signers", self.release)
        for gate in ("git verify-tag", "git cat-file -t", "git merge-base --is-ancestor",
                     "tag does not resolve to the webhook revision", "cargo_version", "web_version",
                     "gitleaks dir --config .gitleaks.toml"):
            self.assertIn(gate, self.release)
        for removed in ("buildctl", "BUILDKIT", "publish-manifest", "verify-amd64",
                        "verify-arm64", "verify-visual-renderer-amd64", ":latest", ":sha-"):
            self.assertNotIn(removed, self.release)
        self.assertEqual(re.findall(r"pipelineTaskName: (.+)", self.release), ["promote-release", "deploy-production"])
        self.assertIn("$(tasks.promote-release.results.image)", self.release)
        self.assertIn("$(tasks.promote-release.results.visual-renderer-image)", self.release)
        self.assertIn('"image=$(params.image)" --config "visualRendererImage=$(params.visual-renderer-image)"', self.release)

    def test_promotion_stages_pinned_crane_for_existing_jq_image(self) -> None:
        task = self.release.split("      - name: promote-release\n", 1)[1].split("      - name: deploy-production", 1)[0]
        self.assertIn("emptyDir: {}", task)
        self.assertIn("image: gcr.io/go-containerregistry/crane:debug@sha256:54b27703e6c602fbd6f95712910e9c8d45d4361a59274bde38aeec943734e424", task)
        self.assertIn("cp /ko-app/crane /tools/crane", task)
        self.assertIn("image: cr.holdenitdown.net/rfhold/general-ci@sha256:6943f91d774980357b24730a14ac4b026325d50962df4b2e15e24c3f43190e5b", task)
        self.assertIn("{ name: crane-tool, mountPath: /tools, readOnly: true }", task)
        self.assertIn('export PATH="/tools:$PATH"', self.script)
        self.assertIn("crane version", self.script)

    def run_promotion(self, **scenario):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            crane = root / "crane"
            crane.write_text(textwrap.dedent('''\
                #!/usr/bin/env python3
                import json, os, sys
                from pathlib import Path
                root = Path(os.environ["FIXTURE_ROOT"])
                scenario = json.loads(os.environ["SCENARIO"])
                args = sys.argv[1:]
                with (root / "calls").open("a") as log:
                    log.write(json.dumps(args) + "\\n")
                command, ref = args[0], args[-1]
                worker = "visual-renderer" in ref
                kind = "worker" if worker else "app"
                digest = "sha256:" + ("2" if worker else "1") * 64
                if command == "version":
                    if scenario.get("version_error"):
                        sys.exit("crane execution unavailable")
                    print("fixture crane")
                elif command == "digest":
                    if ":preview-" in ref:
                        error = scenario.get(kind + "_source_error")
                        if error:
                            sys.exit(error)
                        print(scenario.get(kind + "_digest", digest))
                    else:
                        state = root / (kind + "_copied")
                        alias = scenario.get(kind + "_alias", "missing")
                        if state.exists() or alias == "matching":
                            print(digest if not scenario.get(kind + "_copy_mismatch") else "sha256:" + "3" * 64)
                        elif alias == "conflicting":
                            print("sha256:" + "3" * 64)
                        else:
                            sys.exit("MANIFEST_UNKNOWN" if alias == "missing" else alias)
                elif command == "manifest":
                    manifest = {"manifests": [{"platform": {"architecture": arch, "os": "linux"}} for arch in scenario.get("platforms", ["amd64", "arm64"])]}
                    defect = scenario.get("manifest")
                    if defect == "nested":
                        manifest["unrelated"] = json.loads(json.dumps(manifest))
                        manifest["manifests"] = []
                    elif defect == "os":
                        for entry in manifest["manifests"]:
                            entry["platform"]["os"] = "windows"
                    elif defect == "not-array":
                        manifest["manifests"] = {"platform": {"architecture": "amd64", "os": "linux"}}
                    payload = json.dumps(manifest)
                    print(payload + (" invalid" if defect == "malformed" else payload if defect == "multiple" else ""))
                elif command == "config":
                    arch = args[args.index("--platform") + 1].split("/")[1]
                    config = {"os": "linux", "architecture": arch, "config": {"User": "65532:65532", "Labels": {"org.opencontainers.image.revision": "a" * 40}}}
                    defect = scenario.get(kind + "_config")
                    if defect and defect.startswith("nested-"):
                        config["unrelated"] = json.loads(json.dumps(config))
                        defect = defect.removeprefix("nested-")
                    if defect == "revision":
                        config["config"]["Labels"]["org.opencontainers.image.revision"] = "b" * 40
                    elif defect == "user":
                        config["config"]["User"] = "root"
                    elif defect in ("os", "architecture"):
                        config[defect] = "wrong"
                    payload = json.dumps(config, separators=(",", ":"))
                    print(payload + (" invalid" if defect == "malformed" else payload if defect == "multiple" else ""))
                elif command == "copy":
                    if scenario.get(kind + "_copy_error"):
                        sys.exit(scenario[kind + "_copy_error"])
                    (root / (kind + "_copied")).touch()
                else:
                    sys.exit("unexpected command")
                '''))
            crane.chmod(0o755)
            substitutions = {
                "params.base-image": "registry.test/faktory",
                "params.visual-renderer-base-image": "registry.test/faktory-visual-renderer",
                "params.revision": self.revision,
                "params.release-tag": "v0.1.0",
                "results.image.path": str(root / "app_result"),
                "results.visual-renderer-image.path": str(root / "worker_result"),
            }
            script = self.script
            for name, value in substitutions.items():
                script = script.replace("$(" + name + ")", value)
            result = subprocess.run(["/bin/sh", "-c", script], capture_output=True, text=True,
                                    env={**os.environ, "PATH": directory + os.pathsep + os.environ["PATH"],
                                         "FIXTURE_ROOT": directory, "SCENARIO": json.dumps(scenario)})
            calls = [json.loads(line) for line in (root / "calls").read_text().splitlines()]
            images = [path.read_text() if path.exists() else None for path in (root / "app_result", root / "worker_result")]
            return result, calls, images

    def test_success_copies_exact_digests_after_both_preflights(self) -> None:
        result, calls, images = self.run_promotion()
        self.assertEqual(result.returncode, 0, result.stderr)
        copies = [call for call in calls if call[0] == "copy"]
        self.assertEqual(copies, [["copy", "registry.test/faktory@" + self.app_digest, "registry.test/faktory:v0.1.0"],
                                 ["copy", "registry.test/faktory-visual-renderer@" + self.worker_digest, "registry.test/faktory-visual-renderer:v0.1.0"]])
        before_copy = calls[:calls.index(copies[0])]
        self.assertIn(["digest", "registry.test/faktory:preview-" + self.revision], before_copy)
        self.assertIn(["digest", "registry.test/faktory-visual-renderer:preview-" + self.revision + "-amd64"], before_copy)
        self.assertIn(["digest", "registry.test/faktory:v0.1.0"], before_copy)
        self.assertIn(["digest", "registry.test/faktory-visual-renderer:v0.1.0"], before_copy)
        self.assertIn(["config", "--platform", "linux/arm64", "registry.test/faktory@" + self.app_digest], before_copy)
        self.assertIn(["config", "--platform", "linux/amd64", "registry.test/faktory-visual-renderer@" + self.worker_digest], before_copy)
        self.assertEqual(images, ["registry.test/faktory@" + self.app_digest, "registry.test/faktory-visual-renderer@" + self.worker_digest])

    def test_matching_aliases_are_idempotent(self) -> None:
        for app_alias, worker_alias, copy_count in (("matching", "matching", 0), ("matching", "missing", 1), ("missing", "matching", 1), ("matching", "NAME_UNKNOWN", 1)):
            with self.subTest(app_alias=app_alias, worker_alias=worker_alias):
                result, calls, _ = self.run_promotion(app_alias=app_alias, worker_alias=worker_alias)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(sum(call[0] == "copy" for call in calls), copy_count)

    def test_failed_preflight_never_copies_either_image(self) -> None:
        scenarios = [{kind + "_alias": error} for kind in ("app", "worker")
                     for error in ("conflicting", "UNAUTHORIZED", "DENIED", "network timeout", "404 Not Found")]
        scenarios += [{kind + "_config": defect} for kind in ("app", "worker")
                      for defect in ("revision", "user", "os", "architecture", "nested-revision", "nested-user", "nested-os", "nested-architecture", "malformed", "multiple")]
        scenarios += [{kind + "_source_error": error} for kind in ("app", "worker")
                      for error in ("MANIFEST_UNKNOWN", "UNAUTHORIZED", "network timeout")]
        scenarios += [{kind + "_digest": "sha256:bad"} for kind in ("app", "worker")]
        scenarios += [{"platforms": ["amd64"]}, {"platforms": ["arm64"]}]
        scenarios += [{"manifest": defect} for defect in ("nested", "os", "not-array", "malformed", "multiple")]
        scenarios += [{"version_error": True}]
        for scenario in scenarios:
            with self.subTest(scenario=scenario):
                result, calls, images = self.run_promotion(**scenario)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(any(call[0] == "copy" for call in calls))
                self.assertEqual(images, [None, None])

    def test_digest_mismatch_after_copy_blocks_results(self) -> None:
        for scenario in ({"app_copy_mismatch": True}, {"worker_copy_mismatch": True},
                         {"app_copy_error": "network timeout"}, {"worker_copy_error": "DENIED"}):
            with self.subTest(scenario=scenario):
                result, _, images = self.run_promotion(**scenario)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(images, [None, None])


if __name__ == "__main__":
    unittest.main()
