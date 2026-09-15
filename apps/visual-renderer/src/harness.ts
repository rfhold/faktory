import { Box3, type Camera, type Material, type Mesh, type Object3D, Vector3 } from "three";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";
import { Projection } from "../../../proto/gen/ts/faktory/v1/faktory_pb.js";
import {
  THREE_V2_RECIPE,
  applyCameraSnapshot,
  canonicalCameraSnapshot,
  createCameraRig,
  createRecipeRenderer,
  createRecipeScene,
  type CameraSnapshot,
  type CanonicalViewName,
} from "../../../web/src/viewer/threeRecipe.js";

interface WireCamera {
  target: [number, number, number];
  rotation: [number, number, number, number];
  projection: "perspective" | "orthographic";
  distance: number;
  field_of_view_degrees: number;
  orthographic_scale: number;
}

function disposeObject(object: Object3D): void {
  object.traverse((child) => {
    const mesh = child as Mesh;
    mesh.geometry?.dispose();
    const materials = Array.isArray(mesh.material) ? mesh.material : [mesh.material];
    for (const material of materials) (material as Material | undefined)?.dispose();
  });
}

const canvas = document.querySelector("canvas");
if (!(canvas instanceof HTMLCanvasElement)) throw new Error("missing canvas");
const renderer = createRecipeRenderer({ canvas });
renderer.setPixelRatio(THREE_V2_RECIPE.devicePixelRatio);
renderer.setSize(THREE_V2_RECIPE.width, THREE_V2_RECIPE.height, false);
const { scene } = createRecipeScene();
const rig = createCameraRig(THREE_V2_RECIPE.width / THREE_V2_RECIPE.height);
const target = new Vector3();
let model: Object3D | undefined;
let bounds: Box3 | undefined;

async function settle(camera: Camera): Promise<void> {
  scene.updateMatrixWorld(true);
  camera.updateMatrixWorld(true);
  await renderer.compileAsync(scene, camera);
  for (let frame = 0; frame < 2; frame += 1) {
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
    renderer.render(scene, camera);
  }
  renderer.getContext().finish();
}

function wireSnapshot(camera: WireCamera): CameraSnapshot {
  return {
    target: { x: camera.target[0], y: camera.target[1], z: camera.target[2] },
    rotation: {
      x: camera.rotation[0], y: camera.rotation[1], z: camera.rotation[2], w: camera.rotation[3],
    },
    projection: camera.projection === "orthographic"
      ? Projection.ORTHOGRAPHIC
      : Projection.PERSPECTIVE,
    distance: camera.distance,
    fieldOfViewDegrees: camera.field_of_view_degrees,
    orthographicScale: camera.orthographic_scale,
  };
}

window.faktoryHarness = {
  async load(modelUrl: string): Promise<void> {
    const loaded = await new GLTFLoader().loadAsync(modelUrl);
    if (model) {
      scene.remove(model);
      disposeObject(model);
    }
    model = loaded.scene;
    scene.add(model);
    model.updateMatrixWorld(true);
    bounds = new Box3().setFromObject(model);
    if (bounds.isEmpty()) throw new Error("empty model");
  },
  async renderCanonical(name: CanonicalViewName): Promise<void> {
    if (!bounds) throw new Error("model not loaded");
    const snapshot = canonicalCameraSnapshot(
      name,
      bounds,
      THREE_V2_RECIPE.width / THREE_V2_RECIPE.height,
    );
    if (!snapshot) throw new Error("empty model");
    const camera = applyCameraSnapshot(
      snapshot,
      rig,
      target,
      THREE_V2_RECIPE.width / THREE_V2_RECIPE.height,
    );
    await settle(camera);
  },
  async renderView(cameraSpec: WireCamera): Promise<void> {
    const camera = applyCameraSnapshot(
      wireSnapshot(cameraSpec),
      rig,
      target,
      THREE_V2_RECIPE.width / THREE_V2_RECIPE.height,
    );
    await settle(camera);
  },
};

declare global {
  interface Window {
    faktoryHarness: {
      load(modelUrl: string): Promise<void>;
      renderCanonical(name: CanonicalViewName): Promise<void>;
      renderView(camera: WireCamera): Promise<void>;
    };
  }
}
