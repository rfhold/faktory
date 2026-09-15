import { createEffect, createSignal, onCleanup, onMount, Show } from "solid-js";
import {
  Box3,
  Group,
  OrthographicCamera,
  PerspectiveCamera,
  Scene,
  WebGLRenderer,
  type Camera,
  type Material,
  type Mesh,
  type Object3D,
} from "three";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import {
  THREE_V2_RECIPE,
  applyCameraSnapshot,
  autoframeCameraRig,
  createCameraRig,
  createRecipeRenderer,
  createRecipeScene,
  resizeCameraRig,
  snapshotCamera,
  type CameraSnapshot,
} from "./threeRecipe";

export type { CameraSnapshot } from "./threeRecipe";

interface ModelViewerProps {
  url: string;
  appliedView?: CameraSnapshot;
  onCameraChange: (camera: CameraSnapshot) => void;
  onLoadState: (state: "loading" | "ready" | "error") => void;
}

function disposeObject(object: Object3D) {
  object.traverse((child) => {
    const mesh = child as Mesh;
    mesh.geometry?.dispose();
    const materials = Array.isArray(mesh.material) ? mesh.material : [mesh.material];
    materials.filter(Boolean).forEach((material) => (material as Material).dispose());
  });
}

export function ModelViewer(props: ModelViewerProps) {
  let host!: HTMLDivElement;
  let renderer: WebGLRenderer | undefined;
  let controls: OrbitControls | undefined;
  let perspective: PerspectiveCamera | undefined;
  let orthographic: OrthographicCamera | undefined;
  let activeCamera: Camera | undefined;
  let scene: Scene | undefined;
  let model: Group | undefined;
  let softLights: Group | undefined;
  let studioLights: Group | undefined;
  let animationFrame = 0;
  const [scenePreset, setScenePreset] = createSignal<"soft" | "studio">("soft");
  const [sceneSelectorOpen, setSceneSelectorOpen] = createSignal(false);

  const selectScenePreset = (preset: "soft" | "studio") => {
    setScenePreset(preset);
    setSceneSelectorOpen(false);
    if (!softLights || !studioLights) return;
    softLights.visible = preset === "soft";
    studioLights.visible = preset === "studio";
  };

  const emitCamera = () => {
    if (!controls || !activeCamera || !perspective || !orthographic) return;
    props.onCameraChange(snapshotCamera(activeCamera, { perspective, orthographic }, controls.target));
  };

  const resize = () => {
    if (!renderer || !perspective || !orthographic || !activeCamera) return;
    const width = Math.max(host.clientWidth, 1);
    const height = Math.max(host.clientHeight, 1);
    resizeCameraRig({ perspective, orthographic }, width, height);
    renderer.setSize(width, height, false);
  };

  onMount(() => {
    const recipeScene = createRecipeScene();
    scene = recipeScene.scene;
    softLights = recipeScene.softLights;
    studioLights = recipeScene.studioLights;
    const cameraRig = createCameraRig();
    perspective = cameraRig.perspective;
    orthographic = cameraRig.orthographic;
    activeCamera = perspective;

    renderer = createRecipeRenderer();
    renderer.setPixelRatio(
      Math.min(window.devicePixelRatio, THREE_V2_RECIPE.interactiveDevicePixelRatioLimit),
    );
    renderer.domElement.setAttribute("aria-label", "Interactive 3D model viewer");
    renderer.domElement.setAttribute("role", "img");
    renderer.domElement.tabIndex = 0;
    host.append(renderer.domElement);

    controls = new OrbitControls(activeCamera, renderer.domElement);
    controls.enableDamping = true;
    controls.addEventListener("change", emitCamera);
    selectScenePreset(scenePreset());

    const observer = new ResizeObserver(resize);
    observer.observe(host);
    resize();

    const render = () => {
      animationFrame = requestAnimationFrame(render);
      controls?.update();
      if (renderer && scene && activeCamera) renderer.render(scene, activeCamera);
    };
    render();

    onCleanup(() => {
      observer.disconnect();
      cancelAnimationFrame(animationFrame);
      controls?.dispose();
      if (model) disposeObject(model);
      renderer?.dispose();
      renderer?.domElement.remove();
    });
  });

  createEffect(() => {
    const url = props.url;
    if (!scene || !controls || !activeCamera) return;
    props.onLoadState("loading");
    const loader = new GLTFLoader();
    let cancelled = false;
    void loader.loadAsync(url).then(
      (gltf) => {
        if (cancelled || !scene || !controls || !activeCamera) {
          disposeObject(gltf.scene);
          return;
        }
        if (model) {
          scene.remove(model);
          disposeObject(model);
        }
        model = gltf.scene;
        scene.add(model);
        const bounds = new Box3().setFromObject(model);
        if (perspective && orthographic) {
          const center = autoframeCameraRig(bounds, { perspective, orthographic });
          if (center) {
            controls.target.copy(center);
            controls.update();
            emitCamera();
          }
        }
        props.onLoadState("ready");
      },
      () => {
        if (!cancelled) props.onLoadState("error");
      },
    );
    onCleanup(() => {
      cancelled = true;
    });
  });

  createEffect(() => {
    const view = props.appliedView;
    if (!view || !controls || !perspective || !orthographic || !renderer) return;
    const aspect = Math.max(host.clientWidth, 1) / Math.max(host.clientHeight, 1);
    const nextCamera = applyCameraSnapshot(
      view,
      { perspective, orthographic },
      controls.target,
      aspect,
    );
    activeCamera = nextCamera;
    controls.object = nextCamera;
    resize();
    controls.update();
    emitCamera();
  });

  return (
    <div class="viewer-host" ref={host}>
      <div class="scene-selector">
        <button
          class="scene-selector-trigger"
          type="button"
          aria-expanded={sceneSelectorOpen()}
          onClick={() => setSceneSelectorOpen((open) => !open)}
        >
          {scenePreset() === "soft" ? "Soft" : "Studio"}
          <span aria-hidden="true">{sceneSelectorOpen() ? "-" : "+"}</span>
        </button>
        <Show when={sceneSelectorOpen()}>
          <div class="scene-selector-options" aria-label="Scene">
            <button
              type="button"
              classList={{ active: scenePreset() === "soft" }}
              aria-pressed={scenePreset() === "soft"}
              onClick={() => selectScenePreset("soft")}
            >
              Soft
            </button>
            <button
              type="button"
              classList={{ active: scenePreset() === "studio" }}
              aria-pressed={scenePreset() === "studio"}
              onClick={() => selectScenePreset("studio")}
            >
              Studio
            </button>
          </div>
        </Show>
      </div>
    </div>
  );
}
