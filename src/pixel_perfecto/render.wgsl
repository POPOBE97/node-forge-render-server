struct Params {
    h0: vec4<f32>, h1: vec4<f32>, h2: vec4<f32>, lens: vec4<f32>,
    image: vec4<f32>, view: vec4<f32>, layer: vec4<f32>, options: vec4<f32>,
    compare: vec4<f32>, grid_flags: vec4<f32>, grid: vec4<f32>, output_info: vec4<f32>,
};
@group(0) @binding(0) var tex_a: texture_2d<f32>;
@group(0) @binding(1) var tex_b: texture_2d<f32>;
@group(0) @binding(2) var<uniform> p: Params;
@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u); let y = f32(i & 2u);
    return vec4<f32>(x*2.0-1.0, y*2.0-1.0, 0.0, 1.0);
}
fn over(a: vec4<f32>, b: vec4<f32>) -> vec4<f32> { return a + b*(1.0-a.a); }
fn source_pixel(q: vec2<i32>) -> vec4<f32> {
    if (any(q < vec2<i32>(0)) || any(q >= vec2<i32>(p.image.xy))) { return vec4<f32>(0.0); }
    let color = textureLoad(tex_a,q,0);
    if (p.options.y > 0.5) {return color;}
    return vec4<f32>(color.rgb*color.a,color.a);
}
fn source_sample(pos: vec2<f32>) -> vec4<f32> {
    if (any(pos < vec2<f32>(0.0)) || any(pos >= p.image.xy)) {return vec4<f32>(0.0);}
    // Only this private texture-storage boundary converts public image Y to raster Y.
    let raster = vec2<f32>(pos.x,p.image.y-pos.y)-vec2<f32>(0.5);
    if (p.options.x > 0.5) {return source_pixel(vec2<i32>(floor(raster+vec2<f32>(0.5))));}
    let q=vec2<i32>(floor(raster)); let f=fract(raster);
    return mix(mix(source_pixel(q),source_pixel(q+vec2<i32>(1,0)),f.x),
               mix(source_pixel(q+vec2<i32>(0,1)),source_pixel(q+vec2<i32>(1,1)),f.x),f.y);
}
fn world(pos: vec2<f32>) -> vec2<f32> {return p.view.xy+vec2<f32>(pos.x*p.view.z,-pos.y*p.view.w);}
@fragment fn warp(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let q=vec3<f32>(world(pos.xy)-p.layer.xy,1.0);
    let den=dot(p.h2.xyz,q); var value=vec4<f32>(0.0);
    if (abs(den)>0.0000001) {
        let a=vec2<f32>(dot(p.h0.xyz,q),dot(p.h1.xyz,q))/den;
        let x=a.x;let y=a.y;let r=dot(a,a);let k=1.0+p.lens.x*r+p.lens.y*r*r;
        let d=a*k+vec2<f32>(2.0*p.lens.z*x*y+p.lens.w*(r+2.0*x*x),p.lens.z*(r+2.0*y*y)+2.0*p.lens.w*x*y);
        value=source_sample(p.image.zw+d*p.layer.z)*p.layer.w;
    }
    if (p.output_info.w>0.5) {return over(value,textureLoad(tex_b,vec2<i32>(pos.xy),0));}
    return value;
}
fn cov(d: f32,width: f32) -> f32 {return clamp((width*0.5+0.5/p.grid.z-d)*p.grid.z,0.0,1.0);}
fn grid_color(w: vec2<f32>) -> vec4<f32> {
    if (p.grid_flags.x<0.5) {return vec4<f32>(0.0);}
    let cell=vec2<f32>((w.x+w.y)*0.5,(w.y-w.x)*0.5);var result=vec4<f32>(0.0);
    if (p.grid_flags.y>0.5) {
        let d=abs(cell/p.grid.w-round(cell/p.grid.w))*p.grid.w*sqrt(2.0);
        let a=cov(min(d.x,d.y),1.0/p.grid.z)*p.grid.x*0.45;
        result=vec4<f32>(vec3<f32>(0.4,0.48,0.58)*a,a);
    }
    if (p.grid.z<p.grid.y) {return result;}
    for (var i=0u;i<4u;i+=1u) {
        var offset=vec2<f32>(0.0);var color=vec3<f32>(1.0,0.08,0.12);
        if (i==1u) {offset=vec2<f32>(0.5);color=vec3<f32>(0.12,0.4,1.0);}
        if (i==2u) {offset=vec2<f32>(0.5,0.0);color=vec3<f32>(0.15,1.0,0.4);}
        if (i==3u) {offset=vec2<f32>(0.0,0.5);color=vec3<f32>(0.15,1.0,0.4);}
        let site=round(cell-offset)+offset;let delta=w-vec2<f32>(site.x-site.y,site.x+site.y);
        var a=0.0;
        if (p.grid_flags.z>0.5) {a=cov(length(delta),3.0/p.grid.z);}
        if (p.grid_flags.w>0.5) {
            var d=0.0;
            if (i<2u) {d=abs(abs(delta.x)+abs(delta.y)-select(0.2,0.29,i==1u))/sqrt(2.0);}
            else {
                let sign=select(1.0,-1.0,i==3u);let x=(delta.x+sign*delta.y)/sqrt(2.0);let y=(-sign*delta.x+delta.y)/sqrt(2.0);
                d=abs(length(vec2<f32>(max(abs(x)-0.08,0.0),y))-0.055);
            }
            a=max(a,cov(d,1.0/p.grid.z));
        }
        a*=p.grid.x;result=over(vec4<f32>(color*a,a),result);
    }
    return result;
}
fn encode_srgb(c: vec3<f32>) -> vec3<f32> {
    return select(1.055*pow(max(c,vec3<f32>(0.0)),vec3<f32>(1.0/2.4))-0.055,c*12.92,c<=vec3<f32>(0.0031308));
}
@fragment fn compose(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let a=textureLoad(tex_a,vec2<i32>(pos.xy),0);let b=textureLoad(tex_b,vec2<i32>(pos.xy),0);var color=a;
    let mode=u32(p.options.z);
    if (mode==1u) {
        color=vec4<f32>(0.0);
        if (a.a>0.0 && b.a>0.0) {color=vec4<f32>(abs(a.rgb/a.a-b.rgb/b.a)*p.compare.y,1.0);}
    }
    if (mode==2u && p.options.w>0.5) {color=b;}
    if (mode==3u) {
        let uv=pos.xy/p.output_info.xy;let position=select(uv.x,uv.y,p.compare.z>0.5);
        color=select(a,b,position>=p.compare.x);
    }
    color=over(grid_color(world(pos.xy)),color);
    if (p.compare.w>0.5 && color.a>0.0) {color=vec4<f32>(encode_srgb(color.rgb/color.a)*color.a,color.a);}
    return color;
}
