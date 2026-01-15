vec4 blendPlusLighter(vec4 src, vec4 dst) {
    vec3 color = min(src.rgb + dst.rgb, vec3(1.0));
    float alpha = src.a + (1.0 - src.a) * dst.a;
    return vec4(color, alpha);
}

float box(in vec2 p, in vec2 b, float r) {
    vec2 d = abs(p) - b + r;
    return min(max(d.x, d.y), 0.0) + length(max(d, 0.0)) - r;
}

// 圆形 SDF
float circle(in vec2 p, float r) {
    return length(p) - r;
}

vec4 renderGlassShape(vec4 currentColor, vec2 uvPx, vec2 centerPx, vec2 sizePx, float radiusPx) {

    vec2 halfSizePx = sizePx * 0.5;
    
    vec2 posFromCenter = uvPx - centerPx;
    
    // 方形 SDF (始终以几何中心为基准)
    float boxSdf = box(posFromCenter, halfSizePx, radiusPx);
    float boxNormFactor = min(halfSizePx.x, halfSizePx.y);
    
    // 圆形 SDF (使用 uLightCenter 作为中心，0.5,0.5 表示几何中心)
    // uLightCenter 是相对于 rect 的归一化坐标 (左上角为原点)
    // uvPx 的 Y 轴已经翻转 (左下角为原点)，所以 lightCenter.y 也需要翻转
    vec2 lightCenterPx = vec2(uLightCenter.x, 1.0 - uLightCenter.y) * sizePx;
    vec2 posFromLightCenter = uvPx - lightCenterPx;
    
    float circleRadius = min(halfSizePx.x, halfSizePx.y);
    float circleSdf = circle(posFromLightCenter, circleRadius);
    float circleNormFactor = circleRadius;
    
    // 通过外部传入的 uShapeMorph 控制形状插值
    // uShapeMorph: 0 = 圆形(pressed), 1 = 方形(active)
    float sdf = mix(circleSdf, boxSdf, uShapeMorph);
    float normFactor = mix(circleNormFactor, boxNormFactor, uShapeMorph);

    float finalAlpha = 0.0;

    {
      float lit = 1.0 + sdf / normFactor;
      lit = exp(-lit * lit * uSrc1[0]);
      finalAlpha += lit * uSrc1[1];
    }
    {
      float lit = 1.0 + sdf / normFactor;
      lit = 1.0 - max(pow(1.0 - lit * uSrc2[0], 3.0), 0.0);
      finalAlpha += lit * uSrc2[1];
    }

    {
      float lit = 1.0 + sdf / normFactor;
      lit = 1.0 - smoothstep(0.0, 1.0, abs((lit - uRing1[0]) * uRing1[1]));
      finalAlpha += lit * uRing1[2];
    }
    {
      float lit = 1.0 + sdf / normFactor;
      lit = 1.0 - smoothstep(0.0, 1.0, abs((lit - uRing2[0]) * uRing2[1]));
      finalAlpha += lit * uRing2[2];
    }

    // artistic mapping
    finalAlpha = finalAlpha * finalAlpha;
    finalAlpha = clamp(finalAlpha, 0.0, 1.0);

    {
      float limit = 1.0 + sdf / normFactor;
      finalAlpha *= max(1.0 - pow(limit, 8.0), 0.0);
    }

    // 应用 strength 控制整体光效强度
    finalAlpha *= uStrength;

	vec4 finalColor = uColor;
	finalColor *= finalAlpha;

    return finalColor;
}


void main() {
    vec2 screenUV = vUv;
    vec4 color = vec4(0., 0., 0., 0.);

    vec2 uvPx = screenUV * uGeoPxSize.xy;
    
    vec2 centerPx = uGeoPxSize.xy * 0.5;
    centerPx.y = uGeoPxSize.y - centerPx.y;
    
    color = renderGlassShape(color, uvPx, centerPx, uGeoPxSize.xy, uGeoPxSize.z);

    fragColor = color;
}
