// RAW measurements use LibRaw unpack, never dcraw_process or camera color transforms.
// The separate preview command returns only the camera's embedded JPEG for display.
#include <libraw/libraw.h>
#include <opencv2/core.hpp>
#include <opencv2/imgcodecs.hpp>
#include <opencv2/objdetect/aruco_detector.hpp>
#include <iostream>
#include <stdexcept>
#include <cstring>
static void u32(uint32_t n) { for(int i=0;i<4;i++) std::cout.put((n>>(8*i))&255); }
static void check(int code) { if(code != LIBRAW_SUCCESS) throw std::runtime_error(libraw_strerror(code)); }
int main(int argc,char**argv) { try {
    if(argc<2) throw std::runtime_error("Usage: screen-test-native raw file.CR2 | detect image.pgm | dictionary");
    std::string operation=argv[1];
    if(operation=="display-info" || operation=="preview") {
        if(argc!=3) throw std::runtime_error("RAW path required");
        LibRaw raw; check(raw.open_file(argv[2]));
        if(operation=="preview") {
            check(raw.unpack_thumb());
            const auto &t=raw.imgdata.thumbnail;
            if(t.tformat!=LIBRAW_THUMBNAIL_JPEG || !t.thumb) throw std::runtime_error("RAW has no JPEG display preview");
            std::cout.write(t.thumb,t.tlength);
        } else {
            const auto &s=raw.imgdata.sizes;
            const auto &crop=raw.imgdata.makernotes.canon.DefaultCropAbsolute;
            // Canon's processed JPEG omits sensor border pixels. Do not scale the whole RAW.
            int x=crop.l-s.left_margin, top=crop.t-s.top_margin;
            int w=crop.r-crop.l+1, h=crop.b-crop.t+1;
            if(x<0 || top<0 || w<=1 || h<=1 || x+w>s.width || top+h>s.height)
                throw std::runtime_error("Camera JPEG crop metadata unavailable");
            cv::FileStorage json("",cv::FileStorage::WRITE|cv::FileStorage::MEMORY|cv::FileStorage::FORMAT_JSON);
            json<<"sensor_crop"<<"["<<x<<(s.height-top-h)<<w<<h<<"]"<<"flip"<<s.flip;
            std::cout<<json.releaseAndGetString();
        }
    } else if(operation=="raw") {
        if(argc!=3) throw std::runtime_error("RAW path required");
        LibRaw raw; check(raw.open_file(argv[2])); check(raw.unpack());
        const auto &d=raw.imgdata; const auto &s=d.sizes; const auto &c=d.rawdata.color;
        if(!d.rawdata.raw_image || d.idata.colors!=3 || !d.idata.filters || d.idata.filters==9) throw std::runtime_error("Requires Bayer CFA RAW (Canon CR2)");
        if(c.cblack[4]>2 || c.cblack[5]>2) throw std::runtime_error("Unsupported spatial RAW black table; refusing approximate subtraction");
        int cfa[4], greens=0; double black[4]={0,0,0,0};
        for(int y=0;y<2;y++) for(int x=0;x<2;x++) {
            int color=raw.COLOR(s.top_margin+y,s.left_margin+x); int channel=color;
            if(color==1 || color==3) channel=(greens++==0)?1:3;
            cfa[y*2+x]=channel;
            double value=c.black+c.cblack[color];
            if(c.cblack[4] && c.cblack[5]) value+=c.cblack[6+((s.top_margin+y)%c.cblack[4])*c.cblack[5]+(s.left_margin+x)%c.cblack[5]];
            black[channel]=value;
        }
        cv::FileStorage json("",cv::FileStorage::WRITE|cv::FileStorage::MEMORY|cv::FileStorage::FORMAT_JSON);
        json<<"width"<<(int)s.width<<"height"<<(int)s.height<<"cfa"<<"[";
        for(auto v:cfa) json<<v; json<<"]"<<"black"<<"["; for(auto v:black)json<<v;
        json<<"]"<<"white"<<(double)c.maximum<<"exposure"<<(double)d.other.shutter<<"iso"<<(double)d.other.iso_speed<<"aperture"<<(double)d.other.aperture<<"focal_length"<<(double)d.other.focal_len;
        json<<"camera"<<std::string(d.idata.model)<<"lens"<<std::string(d.lens.Lens)<<"orientation"<<s.flip;
        auto header=json.releaseAndGetString(); std::cout.write("NFR1",4);u32(header.size());std::cout<<header;
        size_t pitch=s.raw_pitch?s.raw_pitch/sizeof(ushort):s.raw_width;
        for(unsigned y=0;y<s.height;y++) for(unsigned x=0;x<s.width;x++) {
            auto v=d.rawdata.raw_image[(y+s.top_margin)*pitch+x+s.left_margin]; std::cout.put(v&255);std::cout.put(v>>8);
        }
    } else {
        auto dictionary=cv::aruco::getPredefinedDictionary(cv::aruco::DICT_6X6_250);
        cv::FileStorage json("",cv::FileStorage::WRITE|cv::FileStorage::MEMORY|cv::FileStorage::FORMAT_JSON);
        if(operation=="dictionary") {
            json<<"markers"<<"[";
            for(int id=0;id<250;id++){cv::Mat marker;cv::aruco::generateImageMarker(dictionary,id,8,marker);json<<"[";for(int y=0;y<8;y++)for(int x=0;x<8;x++)json<<(int)(marker.at<uchar>(y,x)>0);json<<"]";}
            json<<"]";
        } else if(operation=="detect" && argc==3) {
            auto image=cv::imread(argv[2],cv::IMREAD_GRAYSCALE); if(image.empty())throw std::runtime_error("Cannot read detection image");
            cv::aruco::DetectorParameters parameters;parameters.cornerRefinementMethod=cv::aruco::CORNER_REFINE_SUBPIX;
            cv::aruco::ArucoDetector detector(dictionary,parameters);
            std::vector<int> ids;std::vector<std::vector<cv::Point2f>> corners;detector.detectMarkers(image,corners,ids);
            json<<"markers"<<"["; for(size_t i=0;i<ids.size();i++){json<<"{"<<"id"<<ids[i]<<"corners"<<"[";for(auto p:corners[i])json<<"["<<p.x<<p.y<<"]";json<<"]"<<"}";}json<<"]";
        } else throw std::runtime_error("Unknown command");
        std::cout<<json.releaseAndGetString();
    }
    return 0;
} catch(const std::exception&e){std::cerr<<e.what()<<std::endl;return 1;} }
