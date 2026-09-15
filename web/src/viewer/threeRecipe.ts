import {
  AmbientLight,
  Box3,
  Color,
  DirectionalLight,
  Group,
  HemisphereLight,
  MathUtils,
  Matrix4,
  NoToneMapping,
  OrthographicCamera,
  PerspectiveCamera,
  Quaternion,
  Scene,
  SRGBColorSpace,
  Vector3,
  WebGLRenderer,
  type Camera,
  type WebGLRendererParameters,
} from "three";
import { Projection } from "../../../proto/gen/ts/faktory/v1/faktory_pb";

export interface CameraSnapshot {
  target: { x: number; y: number; z: number };
  rotation: { x: number; y: number; z: number; w: number };
  projection: Projection;
  distance: number;
  fieldOfViewDegrees: number;
  orthographicScale: number;
}

export interface CameraRig {
  perspective: PerspectiveCamera;
  orthographic: OrthographicCamera;
}

export interface RecipeScene {
  scene: Scene;
  softLights: Group;
  studioLights: Group;
}

export const CANONICAL_VIEW_NAMES = [
  "isometric",
  "front",
  "back",
  "left",
  "right",
  "top",
  "bottom",
] as const;

export type CanonicalViewName = (typeof CANONICAL_VIEW_NAMES)[number];

interface SourceDirection {
  x: number;
  y: number;
  z: number;
}

const CANONICAL_SOURCE_CAMERAS: Record<
  CanonicalViewName,
  { direction: SourceDirection; screenRight: SourceDirection }
> = {
  isometric: { direction: { x: 1, y: -1, z: 1 }, screenRight: { x: 1, y: 1, z: 0 } },
  front: { direction: { x: 0, y: -1, z: 0 }, screenRight: { x: 1, y: 0, z: 0 } },
  back: { direction: { x: 0, y: 1, z: 0 }, screenRight: { x: -1, y: 0, z: 0 } },
  left: { direction: { x: -1, y: 0, z: 0 }, screenRight: { x: 0, y: -1, z: 0 } },
  right: { direction: { x: 1, y: 0, z: 0 }, screenRight: { x: 0, y: 1, z: 0 } },
  top: { direction: { x: 0, y: 0, z: 1 }, screenRight: { x: 1, y: 0, z: 0 } },
  bottom: { direction: { x: 0, y: 0, z: -1 }, screenRight: { x: 1, y: 0, z: 0 } },
};

export const THREE_V2_RECIPE = Object.freeze({
  id: "three-v2",
  threeVersion: "0.180.0",
  width: 640,
  height: 480,
  devicePixelRatio: 1,
  interactiveDevicePixelRatioLimit: 2,
  background: "#11150f",
  outputColorSpace: SRGBColorSpace,
  toneMapping: NoToneMapping,
  toneMappingExposure: 1,
  antialias: true,
  shadows: false,
  perspectiveFieldOfViewDegrees: 42,
  near: 0.01,
  far: 100_000,
} as const);

const MIN_CAMERA_VALUE = 0.01;
const CANONICAL_FRAME_FILL = 0.82;

export function sourceDirectionToThree(direction: SourceDirection): Vector3 {
  return new Vector3(direction.x, direction.z, -direction.y);
}

export function canonicalCameraSnapshot(
  name: CanonicalViewName,
  bounds: Box3,
  aspect = THREE_V2_RECIPE.width / THREE_V2_RECIPE.height,
): CameraSnapshot | undefined {
  if (bounds.isEmpty()) return undefined;

  const definition = CANONICAL_SOURCE_CAMERAS[name];
  const direction = sourceDirectionToThree(definition.direction).normalize();
  const screenRight = sourceDirectionToThree(definition.screenRight).normalize();
  const screenUp = new Vector3().crossVectors(direction, screenRight).normalize();
  const center = bounds.getCenter(new Vector3());
  const rotation = new Quaternion().setFromRotationMatrix(
    new Matrix4().lookAt(direction, new Vector3(), screenUp),
  );
  const inverseRotation = rotation.clone().invert();
  const tanVertical = Math.tan(
    MathUtils.degToRad(THREE_V2_RECIPE.perspectiveFieldOfViewDegrees) / 2,
  ) * CANONICAL_FRAME_FILL;
  const tanHorizontal = tanVertical * Math.max(aspect, MIN_CAMERA_VALUE);
  let distance = MIN_CAMERA_VALUE;

  for (const x of [bounds.min.x, bounds.max.x]) {
    for (const y of [bounds.min.y, bounds.max.y]) {
      for (const z of [bounds.min.z, bounds.max.z]) {
        const corner = new Vector3(x, y, z).sub(center).applyQuaternion(inverseRotation);
        distance = Math.max(
          distance,
          corner.z + Math.max(Math.abs(corner.x) / tanHorizontal, Math.abs(corner.y) / tanVertical),
        );
      }
    }
  }

  const size = bounds.getSize(new Vector3());
  const orthographicScale = Math.max(
    MIN_CAMERA_VALUE,
    size.length() / CANONICAL_FRAME_FILL,
  );
  return {
    target: { x: center.x, y: center.y, z: center.z },
    rotation: { x: rotation.x, y: rotation.y, z: rotation.z, w: rotation.w },
    projection: Projection.PERSPECTIVE,
    distance,
    fieldOfViewDegrees: THREE_V2_RECIPE.perspectiveFieldOfViewDegrees,
    orthographicScale,
  };
}

export function createRecipeScene(): RecipeScene {
  const scene = new Scene();
  scene.background = new Color(THREE_V2_RECIPE.background);

  const softLights = new Group();
  softLights.add(new AmbientLight(0xdde5d7, 1.4));
  softLights.add(new HemisphereLight(0xf4f7ed, 0x68705f, 2.2));
  for (const position of [new Vector3(4, 5, 6), new Vector3(-4, 3, -6)]) {
    const light = new DirectionalLight(0xe8eee2, 0.9);
    light.position.copy(position);
    softLights.add(light);
  }
  const undersideFill = new DirectionalLight(0xc7d2c2, 0.55);
  undersideFill.position.set(0, -6, 2);
  undersideFill.target.position.set(0, 0, 0);
  softLights.add(undersideFill);
  scene.add(softLights);

  const studioLights = new Group();
  studioLights.add(new AmbientLight(0xdde5d7, 1.8));
  const keyLight = new DirectionalLight(0xfff1cd, 3.5);
  keyLight.position.set(4, 8, 6);
  studioLights.add(keyLight);
  const fillLight = new DirectionalLight(0xc9dcff, 1.4);
  fillLight.position.set(-5, 3, -4);
  studioLights.add(fillLight);
  studioLights.visible = false;
  scene.add(studioLights);

  return { scene, softLights, studioLights };
}

export function createRecipeRenderer(
  parameters: Pick<WebGLRendererParameters, "canvas"> = {},
): WebGLRenderer {
  const renderer = new WebGLRenderer({
    ...parameters,
    antialias: THREE_V2_RECIPE.antialias,
    powerPreference: "high-performance",
  });
  renderer.outputColorSpace = THREE_V2_RECIPE.outputColorSpace;
  renderer.toneMapping = THREE_V2_RECIPE.toneMapping;
  renderer.toneMappingExposure = THREE_V2_RECIPE.toneMappingExposure;
  renderer.shadowMap.enabled = THREE_V2_RECIPE.shadows;
  return renderer;
}

export function createCameraRig(aspect = 1): CameraRig {
  const perspective = new PerspectiveCamera(
    THREE_V2_RECIPE.perspectiveFieldOfViewDegrees,
    aspect,
    THREE_V2_RECIPE.near,
    THREE_V2_RECIPE.far,
  );
  const orthographic = new OrthographicCamera(
    -aspect,
    aspect,
    1,
    -1,
    THREE_V2_RECIPE.near,
    THREE_V2_RECIPE.far,
  );
  perspective.position.set(3, 2, 4);
  orthographic.position.copy(perspective.position);
  return { perspective, orthographic };
}

export function effectiveOrthographicScale(camera: OrthographicCamera): number {
  return (camera.top - camera.bottom) / camera.zoom;
}

export function setEffectiveOrthographicScale(
  camera: OrthographicCamera,
  scale: number,
  aspect: number,
): void {
  const effectiveScale = Math.max(scale, MIN_CAMERA_VALUE);
  const rawScale = effectiveScale * camera.zoom;
  camera.left = (-rawScale * aspect) / 2;
  camera.right = (rawScale * aspect) / 2;
  camera.top = rawScale / 2;
  camera.bottom = -rawScale / 2;
  camera.updateProjectionMatrix();
}

export function resizeCameraRig(rig: CameraRig, width: number, height: number): number {
  const aspect = Math.max(width, 1) / Math.max(height, 1);
  rig.perspective.aspect = aspect;
  rig.perspective.updateProjectionMatrix();
  setEffectiveOrthographicScale(
    rig.orthographic,
    effectiveOrthographicScale(rig.orthographic),
    aspect,
  );
  return aspect;
}

export function perspectiveFootprint(distance: number, fieldOfViewDegrees: number): number {
  const fov = MathUtils.clamp(fieldOfViewDegrees, 1, 179);
  return 2 * Math.max(distance, MIN_CAMERA_VALUE) * Math.tan(MathUtils.degToRad(fov) / 2);
}

export function perspectiveDistance(
  orthographicScale: number,
  fieldOfViewDegrees: number,
): number {
  const fov = MathUtils.clamp(fieldOfViewDegrees, 1, 179);
  return Math.max(orthographicScale, MIN_CAMERA_VALUE)
    / (2 * Math.tan(MathUtils.degToRad(fov) / 2));
}

export function convertCameraProjection(
  camera: CameraSnapshot,
  projection: Projection,
): CameraSnapshot {
  if (camera.projection === projection) return { ...camera };
  if (projection === Projection.ORTHOGRAPHIC) {
    return {
      ...camera,
      projection,
      orthographicScale: perspectiveFootprint(camera.distance, camera.fieldOfViewDegrees),
    };
  }
  return {
    ...camera,
    projection,
    distance: perspectiveDistance(camera.orthographicScale, camera.fieldOfViewDegrees),
  };
}

export function snapshotCamera(
  activeCamera: Camera,
  rig: CameraRig,
  target: Vector3,
): CameraSnapshot {
  return {
    target: { x: target.x, y: target.y, z: target.z },
    rotation: {
      x: activeCamera.quaternion.x,
      y: activeCamera.quaternion.y,
      z: activeCamera.quaternion.z,
      w: activeCamera.quaternion.w,
    },
    projection: activeCamera === rig.orthographic
      ? Projection.ORTHOGRAPHIC
      : Projection.PERSPECTIVE,
    distance: activeCamera.position.distanceTo(target),
    fieldOfViewDegrees: rig.perspective.fov,
    orthographicScale: effectiveOrthographicScale(rig.orthographic),
  };
}

export function applyCameraSnapshot(
  snapshot: CameraSnapshot,
  rig: CameraRig,
  target: Vector3,
  aspect: number,
): Camera {
  const nextCamera = snapshot.projection === Projection.ORTHOGRAPHIC
    ? rig.orthographic
    : rig.perspective;
  target.set(snapshot.target.x, snapshot.target.y, snapshot.target.z);
  nextCamera.quaternion.copy(
    new Quaternion(
      snapshot.rotation.x,
      snapshot.rotation.y,
      snapshot.rotation.z,
      snapshot.rotation.w,
    ).normalize(),
  );
  nextCamera.position
    .copy(target)
    .add(
      new Vector3(0, 0, Math.max(snapshot.distance, MIN_CAMERA_VALUE))
        .applyQuaternion(nextCamera.quaternion),
    );

  rig.perspective.fov = MathUtils.clamp(
    snapshot.fieldOfViewDegrees || THREE_V2_RECIPE.perspectiveFieldOfViewDegrees,
    1,
    179,
  );
  rig.perspective.updateProjectionMatrix();
  if (snapshot.projection === Projection.ORTHOGRAPHIC) rig.orthographic.zoom = 1;
  setEffectiveOrthographicScale(rig.orthographic, snapshot.orthographicScale, aspect);
  return nextCamera;
}

export function autoframeCameraRig(bounds: Box3, rig: CameraRig): Vector3 | undefined {
  if (bounds.isEmpty()) return undefined;
  const center = bounds.getCenter(new Vector3());
  const diagonal = bounds.getSize(new Vector3()).length() || 1;
  const offset = new Vector3(diagonal, diagonal * 0.65, diagonal);
  for (const camera of [rig.perspective, rig.orthographic]) {
    camera.position.copy(center).add(offset);
    camera.lookAt(center);
  }
  rig.perspective.updateProjectionMatrix();
  rig.orthographic.zoom = 1;
  setEffectiveOrthographicScale(
    rig.orthographic,
    perspectiveFootprint(rig.perspective.position.distanceTo(center), rig.perspective.fov),
    rig.perspective.aspect,
  );
  return center;
}
