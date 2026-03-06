// showcase.js — Exercises all Rust-defined APIs from JavaScript.
//
// Loaded as an ES module by the `showcase` example. Every class, module,
// and global function defined in Rust is used here.

import { pi, e, add, multiply, greet, safeDivide, clamp } from "math";

// =========================================================================
// 1. Native module — constants
// =========================================================================

const results = [];

results.push(`pi = ${pi}`);
results.push(`e = ${e}`);

if (Math.abs(pi - 3.141592653589793) > 1e-10) throw new Error("pi wrong");
if (Math.abs(e - 2.718281828459045) > 1e-10) throw new Error("e wrong");

// =========================================================================
// 2. Native module — functions
// =========================================================================

results.push(`add(2, 3) = ${add(2, 3)}`);
results.push(`multiply(4, 5) = ${multiply(4, 5)}`);
results.push(`greet("JS") = ${greet("JS")}`);
results.push(`safeDivide(10, 4) = ${safeDivide(10, 4)}`);
results.push(`clamp(15, 0, 10) = ${clamp(15, 0, 10)}`);

if (add(2, 3) !== 5) throw new Error("add wrong");
if (multiply(4, 5) !== 20) throw new Error("multiply wrong");
if (greet("JS") !== "Hello, JS!") throw new Error("greet wrong");
if (safeDivide(10, 4) !== 2.5) throw new Error("safeDivide wrong");
if (clamp(15, 0, 10) !== 10) throw new Error("clamp wrong");

// Error case
try {
    safeDivide(1, 0);
    throw new Error("expected division-by-zero error");
} catch (err) {
    results.push(`safeDivide(1, 0) threw: "${err.message}"`);
}

// =========================================================================
// 3. Global functions and constants
// =========================================================================

results.push(`appName = ${appName}`);
results.push(`appVersion = ${appVersion}`);
results.push(`formatTimestamp(1709683200) = ${formatTimestamp(1709683200)}`);
results.push(`randomBetween(1, 1) = ${randomBetween(1, 1)}`);

if (appName !== "StarlingMonkey Showcase") throw new Error("appName wrong");
if (appVersion !== "0.1.0") throw new Error("appVersion wrong");
if (randomBetween(5, 5) !== 5) throw new Error("randomBetween wrong");

// =========================================================================
// 4. Classes — basic construction and methods
// =========================================================================

const vec = new Vec2(3, 4);
results.push(`Vec2(3,4).length() = ${vec.length()}`);
results.push(`Vec2(3,4).toString() = ${vec.toString()}`);

if (Math.abs(vec.length() - 5) > 1e-10) throw new Error("Vec2.length wrong");
if (vec.toString() !== "Vec2(3, 4)") throw new Error("Vec2.toString wrong");

// Getters
results.push(`Vec2(3,4).x = ${vec.x}`);
results.push(`Vec2(3,4).y = ${vec.y}`);

if (vec.x !== 3) throw new Error("Vec2.x wrong");
if (vec.y !== 4) throw new Error("Vec2.y wrong");

// Static method
const origin = Vec2.origin();
results.push(`Vec2.origin() = ${origin.toString()}`);
if (origin.toString() !== "Vec2(0, 0)") throw new Error("Vec2.origin wrong");
if (origin.x !== 0) throw new Error("Vec2.origin.x wrong");
if (origin.y !== 0) throw new Error("Vec2.origin.y wrong");

// Instance method with args
const scaled = vec.scale(2);
results.push(`Vec2(3,4).scale(2) = ${scaled.toString()}`);
if (scaled.toString() !== "Vec2(6, 8)") throw new Error("Vec2.scale wrong");
if (scaled.x !== 6) throw new Error("Vec2.scale.x wrong");
if (scaled.y !== 8) throw new Error("Vec2.scale.y wrong");

// RestArgs — variadic static method
const total = Vec2.sum(1, 2, 3, 4, 5);
results.push(`Vec2.sum(1,2,3,4,5) = ${total}`);
if (total !== 15) throw new Error("Vec2.sum wrong");

// =========================================================================
// 5. Inheritance — prototype chain
// =========================================================================

const shape = new Shape("red");
const circle = new Circle("blue", 5);
const rect = new Rect("green", 10, 20);

results.push(`shape.describe() = ${shape.describe()}`);
results.push(`circle.describe() = ${circle.describe()}`);
results.push(`rect.describe() = ${rect.describe()}`);

// Check inherited methods
results.push(`circle.color = ${circle.color}`);
results.push(`circle.area() = ${circle.area()}`);
results.push(`rect.area() = ${rect.area()}`);

if (circle.color !== "blue") throw new Error("Circle.color wrong");
if (Math.abs(circle.area() - Math.PI * 25) > 1e-10) throw new Error("Circle.area wrong");
if (rect.area() !== 200) throw new Error("Rect.area wrong");

// Setter — mutate color via property assignment
shape.color = "yellow";
results.push(`shape.color after set = ${shape.color}`);
if (shape.color !== "yellow") throw new Error("Shape.color setter wrong");

circle.color = "purple";
results.push(`circle.color after set = ${circle.color}`);
if (circle.color !== "purple") throw new Error("Circle.color setter wrong");

// instanceof checks
if (!(circle instanceof Shape)) throw new Error("circle !instanceof Shape");
if (!(circle instanceof Circle)) throw new Error("circle !instanceof Circle");
if (circle instanceof Rect) throw new Error("circle instanceof Rect");
if (!(rect instanceof Shape)) throw new Error("rect !instanceof Shape");

// =========================================================================
// 6. Prototype chain structure
// =========================================================================

if (Object.getPrototypeOf(Circle.prototype) !== Shape.prototype)
    throw new Error("Circle proto chain broken");
if (Object.getPrototypeOf(Rect.prototype) !== Shape.prototype)
    throw new Error("Rect proto chain broken");

results.push("Prototype chain: OK");

// =========================================================================
// Collect results for Rust to verify
// =========================================================================

globalThis.__showcaseResults = results;
globalThis.__showcaseOk = true;
