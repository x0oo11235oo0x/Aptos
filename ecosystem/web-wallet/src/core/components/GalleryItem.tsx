// Copyright (c) Aptos
// SPDX-License-Identifier: Apache-2.0

import React from 'react';
import SquareBox from './SquareBox';
import SquareImage from './SquareImage';

interface GalleryItemProps {
  imageSrc: string;
}

function GalleryItem({
  imageSrc,
}: GalleryItemProps) {
  return (
    <SquareBox cursor="pointer">
      <SquareImage borderRadius=".5rem" src={imageSrc} />
    </SquareBox>
  );
}

export default GalleryItem;
