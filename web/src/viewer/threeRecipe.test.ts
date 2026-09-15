import { describe, expect, it } from "vitest";
import {
  AmbientLight,
  Box3,
  Color,
  DirectionalLight,
  HemisphereLight,
  Quaternion,
  Vector3,
} from "three";
import { Projection } from "../../../proto/gen/ts/faktory/v1/faktory_pb";
import {
  THREE_V2_RECIPE,
  CANONICAL_VIEW_NAMES,
  applyCameraSnapshot,
  autoframeCameraRig,
  canonicalCameraSnapshot,
  convertCameraProjection,
  createCameraRig,
  createRecipeScene,
  effectiveOrthographicScale,
  perspectiveDistance,
  perspectiveFootprint,
  resizeCameraRig,
  setEffectiveOrthographicScale,
  snapshotCamera,
  sourceDirectionToThree,
  type CameraSnapshot,
} from "./threeRecipe";

const baseSnapshot: CameraSnapshot = {
  target: { x: 10, y: 20, z: 30 },
  rotation: { x: 0, y: 0, z: 0, w: 1 },
  projection: Projection.PERSPECTIVE,
  distance: 100,
  fieldOfViewDegrees: 42,
  orthographicScale: 25,
};

function expectDirectionalLight(
  value: unknown,
  color: number,
  intensity: number,
  position: [number, number, number],
): void {
  expect(value).toBeInstanceOf(DirectionalLight);
  const light = value as DirectionalLight;
  expect(light.color.getHex()).toBe(color);
  expect(light.intensity).toBe(intensity);
  expect(light.position.toArray()).toEqual(position);
  expect(light.target.position.toArray()).toEqual([0, 0, 0]);
}

describe("three-v2 recipe", () => {
  it("fixes the browser harness inputs", () => {
    expect(THREE_V2_RECIPE).toMatchObject({
      id: "three-v2",
      threeVersion: "0.180.0",
      width: 640,
      height: 480,
      devicePixelRatio: 1,
      interactiveDevicePixelRatioLimit: 2,
      background: "#11150f",
      toneMappingExposure: 1,
      antialias: true,
      shadows: false,
    });
  });

  it("creates the fixed background and Soft/Studio light groups", () => {
    const { scene, softLights, studioLights } = createRecipeScene();

    expect((scene.background as Color).getHexString()).toBe("11150f");
    expect(softLights.children).toHaveLength(5);
    expect(softLights.visible).toBe(true);
    expect(softLights.children[0]).toBeInstanceOf(AmbientLight);
    expect((softLights.children[0] as AmbientLight).color.getHex()).toBe(0xdde5d7);
    expect((softLights.children[0] as AmbientLight).intensity).toBe(1.4);
    expect(softLights.children[1]).toBeInstanceOf(HemisphereLight);
    expect((softLights.children[1] as HemisphereLight).color.getHex()).toBe(0xf4f7ed);
    expect((softLights.children[1] as HemisphereLight).groundColor.getHex()).toBe(0x68705f);
    expect((softLights.children[1] as HemisphereLight).intensity).toBe(2.2);
    expectDirectionalLight(softLights.children[2], 0xe8eee2, 0.9, [4, 5, 6]);
    expectDirectionalLight(softLights.children[3], 0xe8eee2, 0.9, [-4, 3, -6]);
    expectDirectionalLight(softLights.children[4], 0xc7d2c2, 0.55, [0, -6, 2]);
    expect(studioLights.children).toHaveLength(3);
    expect(studioLights.visible).toBe(false);
    expect(studioLights.children[0]).toBeInstanceOf(AmbientLight);
    expect((studioLights.children[0] as AmbientLight).color.getHex()).toBe(0xdde5d7);
    expect((studioLights.children[0] as AmbientLight).intensity).toBe(1.8);
    expectDirectionalLight(studioLights.children[1], 0xfff1cd, 3.5, [4, 8, 6]);
    expectDirectionalLight(studioLights.children[2], 0xc9dcff, 1.4, [-5, 3, -4]);
  });
});

describe("orthographic camera persistence", () => {
  it("captures effective scale after OrbitControls zoom", () => {
    const rig = createCameraRig(4 / 3);
    setEffectiveOrthographicScale(rig.orthographic, 80, 4 / 3);
    rig.orthographic.zoom = 4;
    rig.orthographic.updateProjectionMatrix();

    const captured = snapshotCamera(rig.orthographic, rig, new Vector3());

    expect(captured.orthographicScale).toBeCloseTo(20);
  });

  it("resets zoom when applying each orthographic view", () => {
    const rig = createCameraRig(4 / 3);
    rig.orthographic.zoom = 7;

    applyCameraSnapshot(
      { ...baseSnapshot, projection: Projection.ORTHOGRAPHIC, orthographicScale: 60 },
      rig,
      new Vector3(),
      4 / 3,
    );
    expect(rig.orthographic.zoom).toBe(1);
    expect(effectiveOrthographicScale(rig.orthographic)).toBeCloseTo(60);

    rig.orthographic.zoom = 4;
    applyCameraSnapshot(
      { ...baseSnapshot, projection: Projection.ORTHOGRAPHIC, orthographicScale: 15 },
      rig,
      new Vector3(),
      4 / 3,
    );
    expect(rig.orthographic.zoom).toBe(1);
    expect(effectiveOrthographicScale(rig.orthographic)).toBeCloseTo(15);
  });

  it("round-trips saved magnification independently of prior zoom", () => {
    const source = createCameraRig(16 / 9);
    source.orthographic.position.set(10, 20, 70);
    source.orthographic.quaternion.copy(new Quaternion(0, 0, 0, 1));
    setEffectiveOrthographicScale(source.orthographic, 120, 16 / 9);
    source.orthographic.zoom = 4;
    const saved = snapshotCamera(source.orthographic, source, new Vector3(10, 20, 30));

    const restored = createCameraRig(16 / 9);
    restored.orthographic.zoom = 9;
    const target = new Vector3();
    const active = applyCameraSnapshot(saved, restored, target, 16 / 9);
    const reloaded = snapshotCamera(active, restored, target);

    expect(saved.orthographicScale).toBeCloseTo(30);
    expect(restored.orthographic.zoom).toBe(1);
    expect(reloaded.orthographicScale).toBeCloseTo(saved.orthographicScale);
    expect(reloaded.target).toEqual(saved.target);
    expect(reloaded.distance).toBeCloseTo(saved.distance);
  });

  it("preserves effective vertical scale while changing aspect", () => {
    const rig = createCameraRig(1);
    rig.orthographic.zoom = 4;
    setEffectiveOrthographicScale(rig.orthographic, 50, 1);

    resizeCameraRig(rig, 1600, 800);

    expect(effectiveOrthographicScale(rig.orthographic)).toBeCloseTo(50);
    expect(rig.orthographic.right - rig.orthographic.left).toBeCloseTo(400);
    expect(rig.orthographic.top - rig.orthographic.bottom).toBeCloseTo(200);
    expect(rig.perspective.aspect).toBe(2);
  });
});

describe("projection conversion", () => {
  it("preserves the vertical footprint in both directions", () => {
    const orthographic = convertCameraProjection(baseSnapshot, Projection.ORTHOGRAPHIC);
    const perspective = convertCameraProjection(orthographic, Projection.PERSPECTIVE);

    expect(orthographic.orthographicScale).toBeCloseTo(
      perspectiveFootprint(baseSnapshot.distance, baseSnapshot.fieldOfViewDegrees),
    );
    expect(perspective.distance).toBeCloseTo(baseSnapshot.distance);
    expect(perspectiveDistance(orthographic.orthographicScale, orthographic.fieldOfViewDegrees))
      .toBeCloseTo(baseSnapshot.distance);
    expect(perspective.target).toEqual(baseSnapshot.target);
    expect(perspective.rotation).toEqual(baseSnapshot.rotation);
  });

  it("applies converted distance along the current orientation", () => {
    const rig = createCameraRig(4 / 3);
    const rotation = new Quaternion().setFromAxisAngle(new Vector3(0, 1, 0), Math.PI / 2);
    const orthographic = {
      ...baseSnapshot,
      rotation: { x: rotation.x, y: rotation.y, z: rotation.z, w: rotation.w },
      projection: Projection.ORTHOGRAPHIC,
      orthographicScale: 254,
    };
    const perspective = convertCameraProjection(orthographic, Projection.PERSPECTIVE);
    const target = new Vector3();

    applyCameraSnapshot(perspective, rig, target, 4 / 3);

    expect(rig.perspective.position.x - target.x).toBeCloseTo(perspective.distance);
    expect(rig.perspective.position.y).toBeCloseTo(target.y);
    expect(rig.perspective.position.z).toBeCloseTo(target.z);
  });
});

describe("bounds autoframe", () => {
  it("gives a large millimetre-scale model a usable orthographic span", () => {
    const rig = createCameraRig(4 / 3);
    const bounds = new Box3(new Vector3(0, 0, 0), new Vector3(254, 254, 254));

    const center = autoframeCameraRig(bounds, rig);

    expect(center?.toArray()).toEqual([127, 127, 127]);
    expect(effectiveOrthographicScale(rig.orthographic)).toBeGreaterThan(254);
    expect(rig.orthographic.position.distanceTo(center!)).toBeCloseTo(
      rig.perspective.position.distanceTo(center!),
    );
  });

  it("allows a saved default view to replace autoframe deterministically", () => {
    const rig = createCameraRig(4 / 3);
    autoframeCameraRig(new Box3(new Vector3(-127, -50, -20), new Vector3(127, 50, 20)), rig);
    const target = new Vector3();
    const savedDefault = {
      ...baseSnapshot,
      projection: Projection.ORTHOGRAPHIC,
      orthographicScale: 40,
    };

    const active = applyCameraSnapshot(savedDefault, rig, target, 4 / 3);

    expect(active).toBe(rig.orthographic);
    expect(target.toArray()).toEqual([10, 20, 30]);
    expect(effectiveOrthographicScale(rig.orthographic)).toBeCloseTo(40);
    expect(rig.orthographic.position.toArray()).toEqual([10, 20, 130]);
  });
});

describe("canonical technical cameras", () => {
  const bounds = new Box3(new Vector3(-2, -3, -5), new Vector3(11, 17, 29));

  it("maps CadQuery Z-up source directions into Three.js Y-up coordinates", () => {
    expect(sourceDirectionToThree({ x: 2, y: 3, z: 5 }).toArray()).toEqual([2, 5, -3]);
    expect(sourceDirectionToThree({ x: 0, y: -1, z: 0 }).toArray()).toEqual([0, 0, 1]);
    expect(sourceDirectionToThree({ x: 0, y: 0, z: 1 }).toArray()[0]).toBeCloseTo(0);
    expect(sourceDirectionToThree({ x: 0, y: 0, z: 1 }).toArray()[1]).toBeCloseTo(1);
    expect(sourceDirectionToThree({ x: 0, y: 0, z: 1 }).toArray()[2]).toBeCloseTo(0);
  });

  it("keeps canonical order, direction, roll, and handedness stable", () => {
    expect(CANONICAL_VIEW_NAMES).toEqual([
      "isometric", "front", "back", "left", "right", "top", "bottom",
    ]);
    const expected: Record<string, { direction: number[]; right: number[] }> = {
      isometric: {
        direction: [-1 / Math.sqrt(3), -1 / Math.sqrt(3), -1 / Math.sqrt(3)],
        right: [1 / Math.sqrt(2), 0, -1 / Math.sqrt(2)],
      },
      front: { direction: [0, 0, -1], right: [1, 0, 0] },
      back: { direction: [0, 0, 1], right: [-1, 0, 0] },
      left: { direction: [1, 0, 0], right: [0, 0, 1] },
      right: { direction: [-1, 0, 0], right: [0, 0, -1] },
      top: { direction: [0, -1, 0], right: [1, 0, 0] },
      bottom: { direction: [0, 1, 0], right: [1, 0, 0] },
    };

    for (const name of CANONICAL_VIEW_NAMES) {
      const snapshot = canonicalCameraSnapshot(name, bounds)!;
      const rotation = new Quaternion(
        snapshot.rotation.x,
        snapshot.rotation.y,
        snapshot.rotation.z,
        snapshot.rotation.w,
      );
      const direction = new Vector3(0, 0, -1).applyQuaternion(rotation).normalize();
      const right = new Vector3(1, 0, 0).applyQuaternion(rotation).normalize();
      direction.toArray().forEach((value, index) => {
        expect(value).toBeCloseTo(expected[name].direction[index]);
      });
      right.toArray().forEach((value, index) => {
        expect(value).toBeCloseTo(expected[name].right[index]);
      });
      expect(new Vector3().crossVectors(right, new Vector3(0, 1, 0).applyQuaternion(rotation))
        .dot(direction)).toBeLessThan(-0.999);
    }
  });

  it("fits every corner of asymmetric geometry in the fixed perspective frustum", () => {
    for (const name of CANONICAL_VIEW_NAMES) {
      const snapshot = canonicalCameraSnapshot(name, bounds)!;
      const rig = createCameraRig(4 / 3);
      const camera = applyCameraSnapshot(snapshot, rig, new Vector3(), 4 / 3);
      camera.updateMatrixWorld(true);
      for (const x of [bounds.min.x, bounds.max.x]) {
        for (const y of [bounds.min.y, bounds.max.y]) {
          for (const z of [bounds.min.z, bounds.max.z]) {
            const projected = new Vector3(x, y, z).project(camera);
            expect(Math.abs(projected.x)).toBeLessThanOrEqual(1);
            expect(Math.abs(projected.y)).toBeLessThanOrEqual(1);
            expect(projected.z).toBeGreaterThanOrEqual(-1);
            expect(projected.z).toBeLessThanOrEqual(1);
          }
        }
      }
    }
  });
});
