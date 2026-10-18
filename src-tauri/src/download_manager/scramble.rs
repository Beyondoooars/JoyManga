use crate::utils;
use image::RgbImage;

pub fn calculate_block_num(scramble_id: i64, id: i64, filename: &str) -> u32 {
    if id < scramble_id {
        0
    } else if id < 268_850 {
        10
    } else {
        let x = if id < 421_926 { 10 } else { 8 };
        let s = format!("{id}{filename}");
        let s = utils::md5_hex(&s);
        let mut block_num = s.chars().last().unwrap() as u32;
        block_num %= x;
        block_num = block_num * 2 + 2;
        block_num
    }
}

/// 拼接图片（使用整块连续切片复制，消除逐像素循环与边界检查开销）
pub fn stitch_img(src_img: &mut RgbImage, block_num: u32) -> RgbImage {
    if block_num == 0 {
        return src_img.clone();
    }
    let (width, height) = src_img.dimensions();
    if width == 0 || height == 0 || height < block_num {
        return src_img.clone();
    }

    let row_bytes = (width as usize) * 3;
    let total_bytes = row_bytes * (height as usize);
    let mut stitched_raw = vec![0u8; total_bytes];
    let src_raw = src_img.as_raw();

    // 计算原图像的高度除以num的余数
    let remainder_height = height % block_num;
    // 将图片切分为block_num块并拼接
    for i in 0..block_num {
        // 计算当前块的标准高度
        let mut block_height = height / block_num;
        // 计算源图像中当前块的Y轴起点位置
        let src_img_y_start = height - (block_height * (i + 1)) - remainder_height;
        // 计算目标图像中当前块的Y轴起点位置
        let mut dst_img_y_start = block_height * i;
        // 第一块需要加上余数高度，以确保拼接完整
        if i == 0 {
            block_height += remainder_height;
        } else {
            dst_img_y_start += remainder_height;
        }

        let block_bytes = (block_height as usize) * row_bytes;
        let src_offset = (src_img_y_start as usize) * row_bytes;
        let dst_offset = (dst_img_y_start as usize) * row_bytes;

        if src_offset + block_bytes <= src_raw.len()
            && dst_offset + block_bytes <= stitched_raw.len()
        {
            stitched_raw[dst_offset..dst_offset + block_bytes]
                .copy_from_slice(&src_raw[src_offset..src_offset + block_bytes]);
        }
    }

    RgbImage::from_raw(width, height, stitched_raw).unwrap_or_else(|| src_img.clone())
}
