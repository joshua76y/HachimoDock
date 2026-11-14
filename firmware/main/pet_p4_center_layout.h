/* Shared drawing/hit-test geometry for the native component launcher. */
#pragma once
#include <stdbool.h>
#include <stddef.h>
enum { PET_CENTER_COLUMNS=2, PET_CENTER_PAGE_SIZE=4,
  PET_CENTER_LEFT=24, PET_CENTER_TOP=86, PET_CENTER_STEP_X=304,
  PET_CENTER_STEP_Y=160, PET_CENTER_CARD_W=288, PET_CENTER_CARD_H=146 };
typedef enum {
  PET_CENTER_DIRECTION_UP,
  PET_CENTER_DIRECTION_DOWN,
  PET_CENTER_DIRECTION_LEFT,
  PET_CENTER_DIRECTION_RIGHT,
} pet_center_direction_t;

/* Move one visible card in the launcher's continuous two-column grid. The
 * renderer may paginate that grid, but navigation must not wrap at a row or
 * invent a neighbour for a partial final row. */
static inline bool pet_center_move(
  size_t selected,size_t count,pet_center_direction_t direction,size_t *target
) {
  if(!target||!count||selected>=count)return false;
  size_t next=selected;
  switch(direction) {
    case PET_CENTER_DIRECTION_UP:
      if(selected<PET_CENTER_COLUMNS)return false;
      next=selected-PET_CENTER_COLUMNS;
      break;
    case PET_CENTER_DIRECTION_DOWN:
      if(selected+PET_CENTER_COLUMNS>=count)return false;
      next=selected+PET_CENTER_COLUMNS;
      break;
    case PET_CENTER_DIRECTION_LEFT:
      if(selected%PET_CENTER_COLUMNS==0)return false;
      next=selected-1;
      break;
    case PET_CENTER_DIRECTION_RIGHT:
      if(selected%PET_CENTER_COLUMNS==PET_CENTER_COLUMNS-1||selected+1>=count)return false;
      next=selected+1;
      break;
    default:
      return false;
  }
  *target=next;
  return true;
}
static inline int pet_center_hit(int x,int y,size_t selected,size_t count) {
  for(int slot=0;slot<PET_CENTER_PAGE_SIZE;slot++) {
    int left=PET_CENTER_LEFT+(slot%PET_CENTER_COLUMNS)*PET_CENTER_STEP_X;
    int top=PET_CENTER_TOP+(slot/PET_CENTER_COLUMNS)*PET_CENTER_STEP_Y;
    size_t index=(selected/PET_CENTER_PAGE_SIZE)*PET_CENTER_PAGE_SIZE+(size_t)slot;
    if(index<count&&x>=left&&x<left+PET_CENTER_CARD_W&&y>=top&&y<top+PET_CENTER_CARD_H)return (int)index;
  }
  return -1;
}
static inline size_t pet_center_page(size_t selected,size_t count,int direction) {
  if(!count)return 0;
  size_t pages=(count+PET_CENTER_PAGE_SIZE-1)/PET_CENTER_PAGE_SIZE;
  size_t page=selected/PET_CENTER_PAGE_SIZE;
  page=direction>0?(page+1)%pages:(page+pages-1)%pages;
  return page*PET_CENTER_PAGE_SIZE;
}
