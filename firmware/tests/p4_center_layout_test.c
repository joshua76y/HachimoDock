#include <assert.h>
#include "pet_p4_center_layout.h"

static void assert_move(
  size_t selected,
  size_t count,
  pet_center_direction_t direction,
  bool expected_moved,
  size_t expected_selected
) {
  size_t next = selected;
  assert(pet_center_move(selected, count, direction, &next) == expected_moved);
  assert(next == expected_selected);
}

int main(void) {
  for(size_t count=0;count<=32;count++)for(size_t selected=0;selected<(count?count:1);selected++) {
    size_t start=selected/4*4;
    for(int slot=0;slot<4;slot++) {
      int x=24+(slot%2)*304,y=86+(slot/2)*160;
      int expected=start+slot<count?(int)start+slot:-1;
      assert(pet_center_hit(x+10,y+10,selected,count)==expected);
      assert(pet_center_hit(x+287,y+145,selected,count)==expected);
      assert(pet_center_hit(x+288,y+146,selected,count)==-1);
    }
    assert(pet_center_hit(320,420,selected,count)==-1);
    size_t next=pet_center_page(selected,count,1),prev=pet_center_page(selected,count,-1);
    assert(!count||(next<count&&prev<count));
    assert(pet_center_page(next,count,-1)==start);
  }
  assert(pet_center_page(24,25,1)==0);
  assert(pet_center_page(0,25,-1)==24);

  /* The launcher is drawn as a two-column grid, so physical joystick moves
   * must select cards by row and column rather than by a flat next/previous
   * sequence. The final partial row has no phantom right/down neighbour. */
  assert_move(0, 6, PET_CENTER_DIRECTION_RIGHT, true, 1);
  assert_move(1, 6, PET_CENTER_DIRECTION_LEFT, true, 0);
  assert_move(0, 6, PET_CENTER_DIRECTION_DOWN, true, 2);
  assert_move(3, 6, PET_CENTER_DIRECTION_DOWN, true, 5);
  assert_move(5, 6, PET_CENTER_DIRECTION_UP, true, 3);
  assert_move(0, 6, PET_CENTER_DIRECTION_LEFT, false, 0);
  assert_move(1, 6, PET_CENTER_DIRECTION_RIGHT, false, 1);
  assert_move(0, 6, PET_CENTER_DIRECTION_UP, false, 0);
  assert_move(5, 6, PET_CENTER_DIRECTION_DOWN, false, 5);
  assert_move(4, 5, PET_CENTER_DIRECTION_RIGHT, false, 4);
  assert_move(0, 0, PET_CENTER_DIRECTION_DOWN, false, 0);
}
