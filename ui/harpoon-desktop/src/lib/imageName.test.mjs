import assert from "node:assert/strict";
import test from "node:test";
import { imageParts, middleShort } from "./imageName.ts";

const TAG = "dep_01M6DPQ543ASWBHW7PW17ZAJ3C";

test("a recorded image splits into repository, tag and id, and every part is complete", () => {
  const p = imageParts(`harpoon/harpoon-ui-test:${TAG} (2f70855fd334)`);
  assert.deepEqual([p.repo, p.tag, p.id], ["harpoon/harpoon-ui-test", TAG, "2f70855fd334"]);
  assert.ok(p.tagShown.includes("…") && p.tagShown.length < TAG.length, "the long tag is shortened in the middle");
  assert.ok(p.tagShown.startsWith(TAG.slice(0, 5)) && p.tagShown.endsWith(TAG.slice(-5)), "start and end stay visible");
});

test("short tags are not touched; a registry port is not a tag; no tag and no id are fine", () => {
  assert.equal(imageParts("app:v1").tagShown, "v1");
  assert.deepEqual(imageParts("localhost:5000/app"), { repo: "localhost:5000/app", tag: null, tagShown: null, id: null });
  assert.equal(imageParts("localhost:5000/app:2").tag, "2");
  assert.equal(middleShort("abc", 5), "abc");
});
