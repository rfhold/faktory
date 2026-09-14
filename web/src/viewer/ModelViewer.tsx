import { createEffect, createSignal, onCleanup, onMount, Show } from "solid-js";
import {
  AmbientLight,
  Box3,
  Color,
  DirectionalLight,
  Group,
  HemisphereLight,
  MathUtils,
  OrthographicCamera,
  PerspectiveCamera,
  Quaternion,
  Scene,
  SRGBColorSpace,
  Vector3,
  WebGLRenderer,
  type Camera,
  type Material,
  type Mesh,
  type Object3D,
} from "three";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import { Projection } from "../../../proto/gen/ts/faktory/v1/faktory_pb";

export interface CameraSnapshot {
  target: { x: number; y: number; z: number };
  rotation: { x: number; y: number; z: number; w: number };
  projection: Projection;
  distance: number;
  fieldOfViewDegrees: number;
  orthographicScale: number;
}

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
    const isOrthographic = activeCamera === orthographic;
    props.onCameraChange({
      target: { x: controls.target.x, y: controls.target.y, z: controls.target.z },
      rotation: {
        x: activeCamera.quaternion.x,
        y: activeCamera.quaternion.y,
        z: activeCamera.quaternion.z,
        w: activeCamera.quaternion.w,
      },
      projection: isOrthographic ? Projection.ORTHOGRAPHIC : Projection.PERSPECTIVE,
      distance: activeCamera.position.distanceTo(controls.target),
      fieldOfViewDegrees: perspective.fov,
      orthographicScale: orthographic.top - orthographic.bottom,
    });
  };

  const resize = () => {
    if (!renderer || !perspective || !orthographic || !activeCamera) return;
    const width = Math.max(host.clientWidth, 1);
    const height = Math.max(host.clientHeight, 1);
    const aspect = width / height;
    perspective.aspect = aspect;
    perspective.updateProjectionMatrix();
    const scale = orthographic.top - orthographic.bottom;
    orthographic.left = (-scale * aspect) / 2;
    orthographic.right = (scale * aspect) / 2;
    orthographic.top = scale / 2;
    orthographic.bottom = -scale / 2;
    orthographic.updateProjectionMatrix();
    renderer.setSize(width, height, false);
  };

  onMount(() => {
    scene = new Scene();
    scene.background = new Color(0x11150f);
    perspective = new PerspectiveCamera(42, 1, 0.01, 100_000);
    orthographic = new OrthographicCamera(-1, 1, 1, -1, 0.01, 100_000);
    orthographic.position.set(3, 2, 4);
    perspective.position.set(3, 2, 4);
    activeCamera = perspective;

    renderer = new WebGLRenderer({ antialias: true, powerPreference: "high-performance" });
    renderer.outputColorSpace = SRGBColorSpace;
    renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
    renderer.domElement.setAttribute("aria-label", "Interactive 3D model viewer");
    renderer.domElement.setAttribute("role", "img");
    renderer.domElement.tabIndex = 0;
    host.append(renderer.domElement);

    controls = new OrbitControls(activeCamera, renderer.domElement);
    controls.enableDamping = true;
    controls.addEventListener("change", emitCamera);
    softLights = new Group();
    softLights.add(new AmbientLight(0xdde5d7, 1.4));
    softLights.add(new HemisphereLight(0xf4f7ed, 0x68705f, 2.2));
    for (const position of [new Vector3(4, 5, 6), new Vector3(-4, 3, -6)]) {
      const light = new DirectionalLight(0xe8eee2, 0.9);
      light.position.copy(position);
      softLights.add(light);
    }
    scene.add(softLights);

    studioLights = new Group();
    studioLights.add(new AmbientLight(0xdde5d7, 1.8));
    const keyLight = new DirectionalLight(0xfff1cd, 3.5);
    keyLight.position.set(4, 8, 6);
    studioLights.add(keyLight);
    const fillLight = new DirectionalLight(0xc9dcff, 1.4);
    fillLight.position.set(-5, 3, -4);
    studioLights.add(fillLight);
    scene.add(studioLights);
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
        if (!bounds.isEmpty()) {
          const center = bounds.getCenter(new Vector3());
          const size = bounds.getSize(new Vector3()).length();
          controls.target.copy(center);
          activeCamera.position.copy(center).add(new Vector3(size || 1, size * 0.65 || 0.65, size || 1));
          activeCamera.lookAt(center);
          controls.update();
          emitCamera();
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
    const nextCamera = view.projection === Projection.ORTHOGRAPHIC ? orthographic : perspective;
    activeCamera = nextCamera;
    controls.object = nextCamera;
    controls.target.set(view.target.x, view.target.y, view.target.z);
    nextCamera.quaternion.copy(
      new Quaternion(view.rotation.x, view.rotation.y, view.rotation.z, view.rotation.w).normalize(),
    );
    nextCamera.position
      .copy(controls.target)
      .add(new Vector3(0, 0, Math.max(view.distance, 0.01)).applyQuaternion(nextCamera.quaternion));
    perspective.fov = MathUtils.clamp(view.fieldOfViewDegrees || 42, 1, 179);
    perspective.updateProjectionMatrix();
    const scale = Math.max(view.orthographicScale, 0.01);
    orthographic.top = scale / 2;
    orthographic.bottom = -scale / 2;
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
