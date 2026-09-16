import re
import unittest
from pathlib import Path


ROOT = Path(__file__).parent


class VisualRendererPipelineTest(unittest.TestCase):
    def setUp(self) -> None:
        self.preview = (ROOT / "faktory-preview.yaml").read_text()
        self.release = (ROOT / "faktory-release.yaml").read_text()
        self.verifier = (ROOT / "verify-visual-renderer.mjs").read_text()

    def test_worker_build_is_amd64_only(self) -> None:
        for pipeline in (self.preview, self.release):
            self.assertEqual(pipeline.count("target=visual-renderer-runtime"), 1)
            self.assertIn("target=visual-renderer-runtime --opt platform=linux/amd64", pipeline)
            unsupported_build = "target=visual-renderer-runtime --opt platform=linux/" + "arm64"
            self.assertNotIn(unsupported_build, pipeline)
            self.assertIn("/faktory-visual-renderer", pipeline)

    def test_renderer_smoke_exercises_complete_multipart_worker_contract(self) -> None:
        for pipeline in (self.preview, self.release):
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
        for pipeline in (self.preview, self.release):
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
        self.assertIn("runAfter: [promote-release, promote-visual-renderer-release]", self.release)

    def test_native_verifier_renders_colored_canonical_response(self) -> None:
        self.assertIn('recipe: "three-v2"', self.verifier)
        self.assertIn('baseColorFactor: [1, 0.02, 0.02, 1]', self.verifier)
        self.assertIn('const names = ["isometric", "front", "back", "left", "right", "top", "bottom"]', self.verifier)
        self.assertIn('image.mime_type !== "image/png"', self.verifier)
        self.assertIn("png.width !== 640 || png.height !== 480", self.verifier)
        self.assertIn("redPixels < 100", self.verifier)

    def test_release_promotes_an_immutable_worker_digest(self) -> None:
        self.assertIn("- name: promote-visual-renderer-release", self.release)
        self.assertIn("source-tag, value: sha-$(params.revision)-amd64", self.release)
        self.assertIn("immutable=\"$(params.base-image)@$digest\"", self.release)
        self.assertIn('grep -Fq \'"User":"65532:65532"\'', self.release)
        self.assertIn('grep -Fq \'"org.opencontainers.image.revision":"$(params.revision)"\'', self.release)


if __name__ == "__main__":
    unittest.main()
