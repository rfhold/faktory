function pad(bytes: Buffer, fill = 0): Buffer {
  const remainder = bytes.length % 4;
  return remainder === 0 ? bytes : Buffer.concat([bytes, Buffer.alloc(4 - remainder, fill)]);
}

function glb(document: Record<string, unknown>, binary?: Buffer): Buffer {
  const json = pad(Buffer.from(JSON.stringify(document)), 0x20);
  const total = 12 + 8 + json.length + (binary ? 8 + binary.length : 0);
  const output = Buffer.alloc(total);
  output.write("glTF", 0, "ascii");
  output.writeUInt32LE(2, 4);
  output.writeUInt32LE(total, 8);
  output.writeUInt32LE(json.length, 12);
  output.writeUInt32LE(0x4e4f534a, 16);
  json.copy(output, 20);
  if (binary) {
    const offset = 20 + json.length;
    output.writeUInt32LE(binary.length, offset);
    output.writeUInt32LE(0x004e4942, offset + 4);
    binary.copy(output, offset + 8);
  }
  return output;
}

export function coloredGlb(): Buffer {
  const positions = new Float32Array([
    -0.5,-0.5,0.5, 0.5,-0.5,0.5, 0.5,0.5,0.5, -0.5,0.5,0.5,
    0.5,-0.5,-0.5, -0.5,-0.5,-0.5, -0.5,0.5,-0.5, 0.5,0.5,-0.5,
    -0.5,0.5,0.5, 0.5,0.5,0.5, 0.5,0.5,-0.5, -0.5,0.5,-0.5,
    -0.5,-0.5,-0.5, 0.5,-0.5,-0.5, 0.5,-0.5,0.5, -0.5,-0.5,0.5,
    0.5,-0.5,0.5, 0.5,-0.5,-0.5, 0.5,0.5,-0.5, 0.5,0.5,0.5,
    -0.5,-0.5,-0.5, -0.5,-0.5,0.5, -0.5,0.5,0.5, -0.5,0.5,-0.5,
  ]);
  const normals = new Float32Array([
    0,0,1, 0,0,1, 0,0,1, 0,0,1, 0,0,-1, 0,0,-1, 0,0,-1, 0,0,-1,
    0,1,0, 0,1,0, 0,1,0, 0,1,0, 0,-1,0, 0,-1,0, 0,-1,0, 0,-1,0,
    1,0,0, 1,0,0, 1,0,0, 1,0,0, -1,0,0, -1,0,0, -1,0,0, -1,0,0,
  ]);
  const indices = new Uint16Array([
    0,1,2, 0,2,3, 4,5,6, 4,6,7, 8,9,10, 8,10,11,
    12,13,14, 12,14,15, 16,17,18, 16,18,19, 20,21,22, 20,22,23,
  ]);
  const positionBytes = Buffer.from(positions.buffer);
  const normalBytes = Buffer.from(normals.buffer);
  const indexBytes = pad(Buffer.from(indices.buffer));
  const binary = Buffer.concat([positionBytes, normalBytes, indexBytes]);
  const document = {
    asset: { version: "2.0", generator: "faktory-test" },
    scene: 0,
    scenes: [{ nodes: [0, 1, 2] }],
    nodes: [
      { mesh: 0, translation: [-1.4, 0, 0] },
      { mesh: 1, translation: [0, 0.5, 0] },
      { mesh: 2, translation: [1.7, -0.25, 0] },
    ],
    meshes: [0, 1, 2].map((material) => ({
      primitives: [{ attributes: { POSITION: 0, NORMAL: 1 }, indices: 2, material }],
    })),
    materials: [
      { pbrMetallicRoughness: { baseColorFactor: [1, 0.03, 0.03, 1], metallicFactor: 0, roughnessFactor: 0.7 } },
      { pbrMetallicRoughness: { baseColorFactor: [0.03, 1, 0.03, 1], metallicFactor: 0, roughnessFactor: 0.7 } },
      { pbrMetallicRoughness: { baseColorFactor: [0.03, 0.03, 1, 1], metallicFactor: 0, roughnessFactor: 0.7 } },
    ],
    buffers: [{ byteLength: binary.length }],
    bufferViews: [
      { buffer: 0, byteOffset: 0, byteLength: positionBytes.length, target: 34962 },
      { buffer: 0, byteOffset: positionBytes.length, byteLength: normalBytes.length, target: 34962 },
      { buffer: 0, byteOffset: positionBytes.length + normalBytes.length, byteLength: indexBytes.length, target: 34963 },
    ],
    accessors: [
      { bufferView: 0, componentType: 5126, count: 24, type: "VEC3", min: [-0.5,-0.5,-0.5], max: [0.5,0.5,0.5] },
      { bufferView: 1, componentType: 5126, count: 24, type: "VEC3" },
      { bufferView: 2, componentType: 5123, count: 36, type: "SCALAR" },
    ],
  };
  return glb(document, binary);
}

export function externalDependencyGlb(): Buffer {
  return glb({
    asset: { version: "2.0" }, scene: 0, scenes: [{ nodes: [0] }], nodes: [{ mesh: 0 }],
    meshes: [{ primitives: [{ attributes: { POSITION: 0 } }] }],
    buffers: [{ uri: "https://example.invalid/private.bin", byteLength: 36 }],
    bufferViews: [{ buffer: 0, byteOffset: 0, byteLength: 36 }],
    accessors: [{ bufferView: 0, componentType: 5126, count: 3, type: "VEC3", min: [0,0,0], max: [1,1,0] }],
  });
}
