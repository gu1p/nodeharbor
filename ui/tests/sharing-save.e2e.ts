import { test, expect, type Locator, type Page } from '@playwright/test';
const tabKey=(page:Page)=>process.platform==='darwin'&&page.context().browser()?.browserType().name()==='webkit'?'Alt+Tab':'Tab';
async function tabTo(page:Page,target:Locator) {
 for(let i=0;i<100;i++) {
  if(await target.evaluate(e=>e===document.activeElement))return;
  await page.keyboard.press(tabKey(page));
 }
 throw new Error('The requested control cannot be reached with the keyboard');
}
async function enter(page:Page,target:Locator,value:string) {
 await tabTo(page,target);await page.keyboard.press('ControlOrMeta+A');await page.keyboard.type(value);
}
test('keyboard setup saves both disks and rules, retains drafts across pages, reopens and prepares',async({page})=>{
 await page.goto('/tests/sharing-save.html');
 const button=(name:string)=>page.getByRole('button',{name,exact:true});
 await tabTo(page,button('Sharing rules'));await page.keyboard.press('Enter');
 await enter(page,page.getByRole('spinbutton',{name:'CPU cores'}),'3');
 for(const [index,key,allocation] of [[1,'w','60'],[2,'d','40']] as const) {
  await tabTo(page,button('Add drive'));await page.keyboard.press('Enter');
  await expect(page.getByRole('combobox',{name:`Drive for disk ${index}`})).toBeFocused();
  await page.keyboard.press(key);await page.keyboard.press(tabKey(page));
  await enter(page,page.getByRole('spinbutton',{name:`Allocation for disk ${index} (GiB)`}),allocation);
  if(index===1)await enter(page,page.getByRole('textbox',{name:'Directory for disk 1'}),'/work/custom');
 }
 await tabTo(page,button('Your machine'));await page.keyboard.press('Enter');
 await tabTo(page,button('Sharing rules'));await page.keyboard.press('Enter');
 await expect(page.getByRole('spinbutton',{name:'Allocation for disk 1 (GiB)'})).toHaveValue('60');
 await expect(page.getByRole('spinbutton',{name:'Allocation for disk 2 (GiB)'})).toHaveValue('40');
 const save=button('Save sharing rules');await expect(save).toBeEnabled();await expect(save).toHaveCSS('cursor','pointer');
 await tabTo(page,save);await page.keyboard.press('Enter');
 const review=page.getByRole('alertdialog',{name:'Save sharing rules and storage?'});
 await expect(review).toContainText('/work/custom');await expect(review).toContainText('100 GiB');
 await expect(button('Keep editing')).toBeFocused();await page.keyboard.press('Enter');await expect(save).toBeFocused();
 await page.keyboard.press('Enter');await tabTo(page,button('Confirm and save'));await page.keyboard.press('Enter');
 await expect(page.getByText('Sharing rules and storage saved',{exact:true})).toBeVisible();
 await page.reload();await tabTo(page,button('Sharing rules'));await page.keyboard.press('Enter');
 await expect(page.getByRole('spinbutton',{name:'CPU cores'})).toHaveValue('3');
 await expect(page.getByRole('textbox',{name:'Directory for disk 1'})).toHaveValue('/work/custom');
 await expect(page.getByRole('spinbutton',{name:'Allocation for disk 2 (GiB)'})).toHaveValue('40');
 await tabTo(page,button('Your machine'));await page.keyboard.press('Enter');await tabTo(page,button('Prepare worker'));await page.keyboard.press('Enter');
 await expect(page.getByRole('heading',{name:'Prepared with saved storage'})).toBeVisible();
});
