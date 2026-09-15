import { createRequire } from "node:module";

const require = createRequire("/opt/visual-renderer/package.json");
const { PNG } = require("pngjs");
const origin = "http://127.0.0.1:8081";

for (let attempt = 0; attempt < 120; attempt += 1) {
  try {
    if ((await fetch(`${origin}/health/ready`)).ok) break;
  } catch {}
  if (attempt === 119) throw new Error("visual renderer did not become ready");
  await new Promise((resolve) => setTimeout(resolve, 500));
}

const pad = (value, fill = 0) => value.length % 4 === 0
  ? value : Buffer.concat([value, Buffer.alloc(4 - value.length % 4, fill)]);
const positions = Buffer.from(new Float32Array([-1, -1, 0, 1, -1, 0, 0, 1, 0]).buffer);
const normals = Buffer.from(new Float32Array([0, 0, 1, 0, 0, 1, 0, 0, 1]).buffer);
const indices = pad(Buffer.from(new Uint16Array([0, 1, 2]).buffer));
const binary = Buffer.concat([positions, normals, indices]);
const document = {
  asset: { version: "2.0" },
  scene: 0,
  scenes: [{ nodes: [0] }],
  nodes: [{ mesh: 0 }],
  meshes: [{ primitives: [{ attributes: { POSITION: 0, NORMAL: 1 }, indices: 2, material: 0 }] }],
  materials: [{
    pbrMetallicRoughness: {
      baseColorFactor: [1, 0.02, 0.02, 1],
      metallicFactor: 0,
      roughnessFactor: 0.7,
    },
  }],
  buffers: [{ byteLength: binary.length }],
  bufferViews: [
    { buffer: 0, byteOffset: 0, byteLength: positions.length, target: 34962 },
    { buffer: 0, byteOffset: positions.length, byteLength: normals.length, target: 34962 },
    { buffer: 0, byteOffset: positions.length + normals.length, byteLength: indices.length, target: 34963 },
  ],
  accessors: [
    { bufferView: 0, componentType: 5126, count: 3, type: "VEC3", min: [-1, -1, 0], max: [1, 1, 0] },
    { bufferView: 1, componentType: 5126, count: 3, type: "VEC3" },
    { bufferView: 2, componentType: 5123, count: 3, type: "SCALAR" },
  ],
};
const json = pad(Buffer.from(JSON.stringify(document)), 0x20);
const glb = Buffer.alloc(12 + 8 + json.length + 8 + binary.length);
glb.write("glTF", 0, "ascii");
glb.writeUInt32LE(2, 4);
glb.writeUInt32LE(glb.length, 8);
glb.writeUInt32LE(json.length, 12);
glb.writeUInt32LE(0x4e4f534a, 16);
json.copy(glb, 20);
const binaryOffset = 20 + json.length;
glb.writeUInt32LE(binary.length, binaryOffset);
glb.writeUInt32LE(0x004e4942, binaryOffset + 4);
binary.copy(glb, binaryOffset + 8);

const spec = Buffer.from(JSON.stringify({ kind: "canonical", recipe: "three-v2" })).toString("base64url");
const names = ["isometric", "front", "back", "left", "right", "top", "bottom"];
const renderAndValidate = async () => {
  const response = await fetch(`${origin}/v1/render`, {
    method: "POST",
    headers: { "content-type": "application/octet-stream", "x-faktory-render-spec": spec },
    body: glb,
  });
  if (!response.ok) throw new Error(`render failed with status ${response.status}`);
  const result = await response.json();
  if (result.recipe !== "three-v2" || result.width !== 640 || result.height !== 480
    || JSON.stringify(result.images?.map(({ name }) => name)) !== JSON.stringify(names)) {
    throw new Error("render response metadata or image names are invalid");
  }
  let redPixels = 0;
  for (const image of result.images) {
    if (image.mime_type !== "image/png") throw new Error("render response MIME type is invalid");
    const png = PNG.sync.read(Buffer.from(image.data, "base64"));
    if (png.width !== 640 || png.height !== 480) throw new Error("render response dimensions are invalid");
    for (let offset = 0; offset < png.data.length; offset += 4) {
      if (png.data[offset] > 80 && png.data[offset] > png.data[offset + 1] * 1.4
        && png.data[offset] > png.data[offset + 2] * 1.4) redPixels += 1;
    }
  }
  if (redPixels < 100) throw new Error("colored GLB was not visible in the rendered images");
};

await renderAndValidate();

const externalDocument = {
  asset: { version: "2.0" },
  scene: 0,
  scenes: [{ nodes: [0] }],
  nodes: [{ mesh: 0 }],
  meshes: [{ primitives: [{ attributes: { POSITION: 0 } }] }],
  buffers: [{ uri: "https://example.invalid/external.bin", byteLength: 36 }],
  bufferViews: [{ buffer: 0, byteOffset: 0, byteLength: 36 }],
  accessors: [{
    bufferView: 0,
    componentType: 5126,
    count: 3,
    type: "VEC3",
    min: [0, 0, 0],
    max: [1, 1, 0],
  }],
};
const externalJson = pad(Buffer.from(JSON.stringify(externalDocument)), 0x20);
const externalGlb = Buffer.alloc(12 + 8 + externalJson.length);
externalGlb.write("glTF", 0, "ascii");
externalGlb.writeUInt32LE(2, 4);
externalGlb.writeUInt32LE(externalGlb.length, 8);
externalGlb.writeUInt32LE(externalJson.length, 12);
externalGlb.writeUInt32LE(0x4e4f534a, 16);
externalJson.copy(externalGlb, 20);

const externalResponse = await fetch(`${origin}/v1/render`, {
  method: "POST",
  headers: { "content-type": "application/octet-stream", "x-faktory-render-spec": spec },
  body: externalGlb,
});
const externalBody = await externalResponse.arrayBuffer();
if (externalResponse.status !== 422 || externalBody.byteLength > 1024) {
  throw new Error(`external-resource rejection failed with status ${externalResponse.status}`);
}

await renderAndValidate();
